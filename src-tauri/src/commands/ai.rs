use crate::commands::operator;
use crate::commands::store::AppState;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::ipc::Channel;
use tauri::State;

// ---------------------------------------------------------------------------
// Credential handling — keys live in the OS credential store, never on disk.
// ---------------------------------------------------------------------------

fn credential_service_name(provider: &str) -> Result<String, String> {
    let allowed = ["openrouter", "nvidia", "ollama", "compatible"];
    if !allowed.contains(&provider) {
        return Err(format!("unknown provider '{provider}'"));
    }
    Ok(format!("cameo_provider_{provider}"))
}

fn keyring_entry(provider: &str) -> Result<keyring::Entry, String> {
    let service = credential_service_name(provider)?;
    // Validate the provider is one of the allowed AI providers
    let allowed = ["openrouter", "nvidia", "ollama", "compatible"];
    if !allowed.contains(&provider) {
        return Err(format!("unknown provider '{provider}'"));
    }
    keyring::Entry::new(&service, "api_key").map_err(|e| format!("credential store error: {e}"))
}

#[tauri::command]
pub async fn save_provider_key(provider: String, key: String) -> Result<bool, String> {
    if key.trim().len() < 8 {
        return Err("API key looks too short to be valid".into());
    }
    let provider = provider;
    tauri::async_runtime::spawn_blocking(move || {
        let entry = keyring_entry(&provider)?;
        entry
            .set_password(key.trim())
            .map(|_| true)
            .map_err(|e| format!("failed to save credential: {e}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn delete_provider_key(provider: String) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let entry = keyring_entry(&provider)?;
        match entry.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(format!("failed to delete credential: {e}")),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn has_provider_key(provider: String) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let entry = keyring_entry(&provider)?;
        Ok(entry.get_password().is_ok())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn get_key_sync(provider: &str) -> Result<Option<String>, String> {
    match keyring_entry(provider) {
        Ok(entry) => match entry.get_password() {
            Ok(k) => Ok(Some(k)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("credential store error: {e}")),
        },
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GenOpts {
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub proxy_profile_id: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Opt-in agent mode: allow the model to call approved operator tools.
    #[serde(default)]
    pub agent_tools: Option<bool>,
    /// Hard cap on tool rounds (each round is one extra model call). Clamped server-side.
    #[serde(default)]
    pub agent_max_rounds: Option<u32>,
    /// If true, tools the provider marks safe (fs_read, net_fetch, net_search
    /// when gated on) may run without a per-call approval click. Destructive
    /// tools always require an explicit `chat_tool_decision`.
    #[serde(default)]
    pub agent_auto_approve_safe: Option<bool>,
    /// Autopilot: auto-approve EVERY tool call, including destructive and
    /// device actions. Only the frontend's explicit hands-free toggle sets
    /// this; every auto-approved call is still audit-logged.
    #[serde(default)]
    pub agent_auto_approve_all: Option<bool>,
}

#[derive(Serialize, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub capabilities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pricing_in_per_m: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pricing_out_per_m: Option<f64>,
}

#[derive(Serialize, Clone)]
pub struct ProviderStatus {
    pub ok: bool,
    pub message: String,
}

#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEvent {
    Delta { text: String },
    /// A slice of the model's reasoning/thinking trace (reasoning models
    /// spend a long silent stretch here — surfacing it removes the "stuck"
    /// feeling and shows the process).
    Thinking { text: String },
    /// A one-line phase note for the process timeline
    /// ("Contacting provider…", "Retrying…", "Running tool…”).
    Status { message: String },
    Usage { prompt_tokens: u64, completion_tokens: u64 },
    /// The agent requested an operator tool call.
    Tool {
        approval_id: String,
        call_id: String,
        name: String,
        arguments: Value,
        preview: String,
        requires_approval: bool,
    },
    /// A pending approval was answered (approved or denied).
    ToolApproval {
        approval_id: String,
        approved: bool,
    },
    /// The operator finished executing a tool (or it was denied).
    ToolResult {
        approval_id: String,
        call_id: String,
        name: String,
        ok: bool,
        output: String,
    },
    Done,
    Error { message: String },
}

fn provider_base(provider: &str, opts: &GenOpts) -> Result<String, String> {
    match provider {
        "openrouter" => Ok("https://openrouter.ai/api/v1".into()),
        "nvidia" => Ok("https://integrate.api.nvidia.com/v1".into()),
        "ollama" => Ok("http://localhost:11434".into()),
        "compatible" => opts
            .base_url
            .as_ref()
            .map(|u| u.trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty())
            .ok_or_else(|| "compatible provider requires a base URL in Settings".into()),
        other => Err(format!("unknown provider '{other}'")),
    }
}

fn http_error(status: u16, body: &str) -> String {
    match status {
        401 | 403 => "Authentication failed — check the API key in Settings (and remaining credits/permissions on the provider site).".to_string(),
        429 => "Rate limited by the provider — slow down or check your plan limits.".to_string(),
        404 => {
            if body.contains("Not found for account") {
                format!("This model is not enabled for your NVIDIA account tier (HTTP 404). Pick a served model that your account can use — e.g. 'openai/gpt-oss-20b' worked in the last provider check. Details: {}", truncate(body, 200))
            } else {
                format!("Endpoint or model not found (HTTP 404): {}", truncate(body, 200))
            }
        }
        402 => format!("Insufficient credits (HTTP 402): {}", truncate(body, 200)),
        _ => format!("Provider error HTTP {status}: {}", truncate(body, 300)),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}

fn request_timeout(opts: &GenOpts) -> Duration {
    Duration::from_millis(opts.timeout_ms.unwrap_or(600_000).clamp(1_000, 600_000))
}

fn build_client(proxy_profile_id: Option<&str>, state: &AppState, request_timeout: Duration) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(request_timeout)
        // Keep repeated chats on warm connections instead of paying a fresh
        // TLS + TCP handshake on the first byte of every message.
        .pool_max_idle_per_host(8)
        .pool_idle_timeout(Duration::from_secs(60))
        .tcp_keepalive(Duration::from_secs(30))
        .user_agent("CAMEO-AI/0.1 (+tauri)");
    if let Some(pid) = proxy_profile_id {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let row = db
            .query_row(
                "SELECT kind, host, port, username, has_password, enabled FROM proxy_profiles WHERE id = ?1 AND enabled = 1",
                [pid],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, i64>(4)?,
                    ))
                },
            )
            .map_err(|_| format!("proxy profile '{pid}' not found or not enabled"))?;
        drop(db);
        let (kind, host, port, username, has_password) = row;
        let mut url = match kind.as_str() {
            "http" => format!("http://{host}:{port}"),
            "https" => format!("https://{host}:{port}"),
            "socks5" => format!("socks5://{host}:{port}"),
            other => return Err(format!("unsupported proxy kind '{other}'")),
        };
        if let Some(user) = username.filter(|u| !u.is_empty()) {
            let secret: String = if has_password == 1 {
                keyring::Entry::new(&format!("cameo_proxy_{pid}"), "password")
                    .ok()
                    .and_then(|e| e.get_password().ok())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            url = format!(
                "{}://{}:{}@{}:{}",
                match kind.as_str() { "socks5" => "socks5", "https" => "https", _ => "http" },
                user,
                secret,
                host,
                port
            );
        }
        let proxy = reqwest::Proxy::all(&url).map_err(|e| format!("invalid proxy URL: {e}"))?;
        builder = builder.proxy(proxy);
    } else {
        builder = builder.no_proxy();
    }
    builder.build().map_err(|e| format!("network client error: {e}"))
}

// ---------------------------------------------------------------------------
// CAMEO proxy routing + model-tier fallback helpers (chat has *tests* below)
// ---------------------------------------------------------------------------

/// Resolve which of CAMEO's own proxy profiles AI/model calls should use:
/// 1. a profile pinned via the `ai_proxy_profile_id` pref (if still enabled),
/// 2. otherwise the first enabled proxy profile (oldest first).
/// Returns None when no proxy is available.
fn active_proxy_id(state: &AppState) -> Option<String> {
    let db = state.db.lock().ok()?;
    if let Ok(Some(pinned)) = db
        .query_row("SELECT value FROM prefs WHERE key = 'ai_proxy_profile_id'", [], |r| r.get::<_, String>(0))
        .optional()
    {
        let enabled: i64 = db
            .query_row("SELECT enabled FROM proxy_profiles WHERE id = ?1", [&pinned], |r| r.get(0))
            .unwrap_or(0);
        if enabled == 1 {
            return Some(pinned);
        }
    }
    db.query_row(
        "SELECT id FROM proxy_profiles WHERE enabled = 1 ORDER BY created_at ASC LIMIT 1",
        [],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// Cached HTTP clients per (proxy, timeout) so streaming chats and probes do
/// not pay a fresh TLS handshake on every message.
static CLIENT_CACHE: std::sync::Mutex<Option<std::collections::HashMap<String, reqwest::Client>>> =
    std::sync::Mutex::new(None);

fn cached_client(proxy_id: Option<&str>, state: &AppState, request_timeout: Duration) -> Result<reqwest::Client, String> {
    let key = format!(
        "{}|{}",
        proxy_id.unwrap_or("direct"),
        request_timeout.as_millis()
    );
    {
        let guard = CLIENT_CACHE.lock().map_err(|_| "client cache poisoned")?;
        if let Some(clients) = guard.as_ref() {
            if let Some(c) = clients.get(&key) {
                return Ok(c.clone());
            }
        }
    }
    let client = build_client(proxy_id, state, request_timeout)?;
    if let Ok(mut guard) = CLIENT_CACHE.lock() {
        guard
            .get_or_insert_with(std::collections::HashMap::new)
            .insert(key, client.clone());
    }
    Ok(client)
}

fn prefs_get(state: &AppState, key: &str) -> Option<String> {
    let db = state.db.lock().ok()?;
    db.query_row("SELECT value FROM prefs WHERE key = ?1", [key], |r| r.get::<_, String>(0)).ok()
}

fn prefs_set(state: &AppState, key: &str, value: &str) {
    if let Ok(db) = state.db.lock() {
        let _ = db.execute(
            "INSERT INTO prefs (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        );
    }
}

fn verified_model_pref_key(provider: &str) -> String {
    format!("chat_model_verified_{provider}")
}

/// Remember a model that provably answers chat completions for this provider.
fn persist_verified_model(state: &AppState, provider: &str, model: &str) {
    prefs_set(state, &verified_model_pref_key(provider), model);
}

fn gated_models_pref_key(provider: &str) -> String {
    format!("chat_model_gated_{provider}")
}

/// Models this key was already refused for (NVIDIA tier gate). Newline-separated
/// so the pref stays readable in the DB.
fn parse_gated_models(raw: Option<String>) -> Vec<String> {
    raw.unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn gated_models(state: &AppState, provider: &str) -> Vec<String> {
    parse_gated_models(prefs_get(state, &gated_models_pref_key(provider)))
}

/// Record that this key cannot serve `model`, so later chats skip the doomed
/// request instead of paying for the 404 and re-printing the notice.
fn remember_gated_model(state: &AppState, provider: &str, model: &str) {
    let model = model.trim();
    if model.is_empty() {
        return;
    }
    let mut models = gated_models(state, provider);
    if models.iter().any(|m| m == model) {
        return;
    }
    models.push(model.to_string());
    // Bounded: a tier upgrade only has to clear the newest 20 to be retried.
    if models.len() > 20 {
        models.drain(..models.len() - 20);
    }
    prefs_set(state, &gated_models_pref_key(provider), &models.join("\n"));
}

/// Make the working model the selected one, so the UI and the next launch stop
/// asking for a model this key cannot serve.
fn persist_model_selection(state: &AppState, provider: &str, model: &str) {
    let value = json!({ "provider": provider, "model": model }).to_string();
    prefs_set(state, "model_selection", &value);
}

/// Pure helper (unit-tested): the model a chat should start on. A model this key
/// was already refused for is swapped for the verified one; an explicit choice
/// that has not been refused is always honoured.
fn preferred_start_model(requested: &str, verified: Option<&str>, gated: &[String]) -> String {
    let requested = requested.trim();
    let was_gated = gated.iter().any(|g| g == requested);
    match verified.map(str::trim) {
        Some(v) if was_gated && !v.is_empty() && v != requested => v.to_string(),
        _ => requested.to_string(),
    }
}

/// Pure helper (unit-tested): build the ordered fallback candidate list.
/// Order: last verified model, well-known lightweight defaults, then catalog ids.
/// Always excludes the model that just failed and dedupes; capped at `cap`.
fn fallback_candidates(verified: Option<&str>, catalog: &[String], failed: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let push = |c: Option<String>, out: &mut Vec<String>| {
        if let Some(c) = c {
            let c = c.trim().to_string();
            if !c.is_empty() && c != failed && !out.iter().any(|x| x == &c) {
                out.push(c);
            }
        }
    };
    push(verified.map(|s| s.to_string()), &mut out);
    for c in catalog {
        push(Some(c.clone()), &mut out);
    }
    out
}

/// Probe whether a model answers a *tiny* chat completion for this key.
async fn probe_model_serves(
    client: &reqwest::Client,
    base: &str,
    key: Option<&str>,
    model: &str,
) -> bool {
    let body = json!({
        "model": model,
        "messages": [{ "role": "user", "content": "ping" }],
        "max_tokens": 8,
        "temperature": 0,
        "stream": false,
    });
    let mut req = client.post(format!("{base}/chat/completions")).json(&body);
    if let Some(k) = key {
        req = req.bearer_auth(k);
    }
    match req.send().await {
        Ok(r) => r.status().as_u16() == 200,
        Err(_) => false,
    }
}

/// WHEN the provider lists a model but the account tier cannot run it
/// (NVIDIA NIM "Not found for account"), probe candidates and return the first
/// model that actually serves chat completions. Persists the winner as the
/// verified model so future fallbacks are instant.
async fn find_served_model(
    client: &reqwest::Client,
    provider: &str,
    base: &str,
    key: Option<&str>,
    failed: &str,
    state: &AppState,
) -> Option<String> {
    if provider != "nvidia" {
        return None;
    }
    // Strongest reasoning first: DeepSeek R1 leads, V4.1-flash follows, then
    // lightweight fallbacks. The first model that actually serves on this
    // key's tier wins and is remembered as the verified model.
    const KNOWN_LIGHTWEIGHT: [&str; 4] = [
        "deepseek-ai/deepseek-r1",
        "deepseek-ai/deepseek-v4.1-flash",
        "openai/gpt-oss-20b",
        "mistralai/mistral-7b-instruct-v0.3",
    ];
    let seeds: Vec<String> = KNOWN_LIGHTWEIGHT.iter().map(|s| s.to_string()).collect();
    let verified = prefs_get(state, &verified_model_pref_key(provider));
    // Cheapest path first: probe the last verified model and the known
    // lightweight defaults BEFORE paying for a catalog fetch.
    for cand in fallback_candidates(verified.as_deref(), &seeds, failed) {
        if probe_model_serves(client, base, key, &cand).await {
            persist_verified_model(state, provider, &cand);
            return Some(cand);
        }
    }
    // Nothing pre-known served: fetch the catalog and probe its ids.
    let mut catalog: Vec<String> = Vec::new();
    let mut req = client.get(format!("{base}/models"));
    if let Some(k) = key {
        req = req.bearer_auth(k);
    }
    if let Ok(resp) = req.send().await {
        if resp.status().as_u16() == 200 {
            if let Ok(v) = resp.json::<serde_json::Value>().await {
                if let Some(items) = v["data"].as_array() {
                    for m in items {
                        if let Some(id) = m["id"].as_str() {
                            catalog.push(id.to_string());
                        }
                    }
                }
            }
        }
    }
    // Bound the catalog sweep: every probe is a full round-trip, so an
    // unbounded sweep over a large NVIDIA catalog stalls the first token
    // for minutes. Five post-seed probes is enough to recover; beyond that
    // the user must pick a served model explicitly.
    for cand in catalog_probe_list(&catalog, failed, 5) {
        if probe_model_serves(client, base, key, &cand).await {
            persist_verified_model(state, provider, &cand);
            return Some(cand);
        }
    }
    None
}

/// Pure helper (unit-tested): the bounded catalog sweep for the tier-gate
/// fallback. Excludes the failed model, dedupes, capped at `cap`.
fn catalog_probe_list(catalog: &[String], failed: &str, cap: usize) -> Vec<String> {
    fallback_candidates(None, catalog, failed).into_iter().take(cap).collect()
}

// ---------------------------------------------------------------------------
// Model discovery + connection test
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn list_models(provider: String, state: State<'_, AppState>) -> Result<Vec<ModelInfo>, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || list_models_sync(&provider, &state))
        .await
        .map_err(|e| e.to_string())?
}

fn list_models_sync(provider: &str, state: &AppState) -> Result<Vec<ModelInfo>, String> {
    let base = provider_base(provider, &GenOpts::default())?;
    let proxy_id = active_proxy_id(state);
    let client = cached_client(proxy_id.as_deref(), state, Duration::from_secs(600))?;
    match provider {
        "openrouter" => {
            let resp = tauri::async_runtime::block_on(async {
                client
                    .get(format!("{base}/models"))
                    .send()
                    .await
                    .map_err(|e| format!("network error reaching OpenRouter: {e}"))
            })?;
            let status = resp.status().as_u16();
            let body = tauri::async_runtime::block_on(async { resp.text().await.unwrap_or_default() });
            if status != 200 {
                return Err(http_error(status, &body));
            }
            let v: Value = serde_json::from_str(&body).map_err(|e| format!("bad model list response: {e}"))?;
            let mut out = Vec::new();
            if let Some(items) = v["data"].as_array() {
                for m in items {
                    let id = m["id"].as_str().unwrap_or_default().to_string();
                    if id.is_empty() {
                        continue;
                    }
                    let name = m["name"].as_str().unwrap_or(&id).to_string();
                    let ctx = m["context_length"].as_u64();
                    let pricing_in = m["pricing"]["prompt"].as_str().and_then(|s| s.parse::<f64>().ok());
                    let pricing_out = m["pricing"]["completion"].as_str().and_then(|s| s.parse::<f64>().ok());
                    let mut caps = vec!["text".to_string(), "streaming".to_string()];
                    if let Some(modality) = m["architecture"]["input_modalities"].as_array() {
                        if modality.iter().any(|x| x.as_str() == Some("image")) {
                            caps.push("vision".into());
                        }
                    } else if let Some(modality) = m["architecture"]["modality"].as_str() {
                        if modality.contains("image") {
                            caps.push("vision".into());
                        }
                    }
                    if let Some(params) = m["supported_parameters"].as_array() {
                        if params.iter().any(|x| x.as_str() == Some("tools")) {
                            caps.push("tools".into());
                        }
                        if params.iter().any(|x| x.as_str() == Some("structured_outputs")) {
                            caps.push("structured-output".into());
                        }
                    }
                    out.push(ModelInfo {
                        id,
                        name,
                        provider: provider.into(),
                        capabilities: caps,
                        context_length: ctx,
                        pricing_in_per_m: pricing_in.map(|p| p * 1_000_000.0),
                        pricing_out_per_m: pricing_out.map(|p| p * 1_000_000.0),
                    });
                }
            }
            Ok(out)
        }
        "nvidia" | "compatible" => {
            let key = get_key_sync(provider)?;
            let mut req = client.get(format!("{base}/models"));
            if let Some(k) = key {
                req = req.bearer_auth(k);
            }
            let url_used = format!("{base}/models");
            let resp = tauri::async_runtime::block_on(async {
                req.send().await.map_err(|e| {
                    if provider == "nvidia" {
                        format!("network error reaching NVIDIA NIM: {e}")
                    } else {
                        format!("network error reaching endpoint {url_used}: {e}")
                    }
                })
            })?;
            let status = resp.status().as_u16();
            let body = tauri::async_runtime::block_on(async { resp.text().await.unwrap_or_default() });
            if status != 200 {
                return Err(http_error(status, &body));
            }
            let v: Value = serde_json::from_str(&body).map_err(|e| format!("bad model list response: {e}"))?;
            let mut out = Vec::new();
            if let Some(items) = v["data"].as_array() {
                for m in items {
                    let id = m["id"].as_str().unwrap_or_default().to_string();
                    if id.is_empty() {
                        continue;
                    }
                    out.push(ModelInfo {
                        name: id.clone(),
                        id,
                        provider: provider.into(),
                        capabilities: vec!["text".into(), "streaming".into()],
                        context_length: None,
                        pricing_in_per_m: None,
                        pricing_out_per_m: None,
                    });
                }
            }
            Ok(out)
        }
        "ollama" => {
            let resp = tauri::async_runtime::block_on(async {
                client
                    .get(format!("{base}/api/tags"))
                    .send()
                    .await
                    .map_err(|e| format!("cannot reach Ollama at {base} — is 'ollama serve' running? ({e})"))
            })?;
            let status = resp.status().as_u16();
            let body = tauri::async_runtime::block_on(async { resp.text().await.unwrap_or_default() });
            if status != 200 {
                return Err(http_error(status, &body));
            }
            let v: Value = serde_json::from_str(&body).map_err(|e| format!("bad Ollama response: {e}"))?;
            let mut out = Vec::new();
            if let Some(items) = v["models"].as_array() {
                for m in items {
                    let id = m["name"].as_str().unwrap_or_default().to_string();
                    if id.is_empty() {
                        continue;
                    }
                    let details = &m["details"];
                    let mut caps = vec!["text".into()];
                    if details["families"].as_array().is_some_and(|f| {
                        f.iter().any(|x| x.as_str().map(|s| s.contains("clip")).unwrap_or(false))
                    }) {
                        caps.push("vision".into());
                    }
                    out.push(ModelInfo {
                        name: id.clone(),
                        id,
                        provider: provider.into(),
                        capabilities: caps,
                        context_length: None,
                        pricing_in_per_m: None,
                        pricing_out_per_m: None,
                    });
                }
            }
            Ok(out)
        }
        other => Err(format!("unknown provider '{other}'")),
    }
}


#[tauri::command]
pub async fn test_connection(provider: String, state: State<'_, AppState>) -> Result<ProviderStatus, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        match list_models_sync(&provider, &state) {
            Ok(models) => {
                if models.is_empty() {
                    Ok(ProviderStatus { ok: false, message: "Connection ok but no models were returned".into() })
                } else {
                    Ok(ProviderStatus { ok: true, message: format!("Connected — {} models available", models.len()) })
                }
            }
            Err(e) => Ok(ProviderStatus { ok: false, message: e }),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------------------
// Chat generation (streamed to the frontend via a Channel)
// ---------------------------------------------------------------------------

/// Build a provider-specific payload and reject blank models before any network request.
fn build_chat_payload(
    provider: &str,
    model: &str,
    messages: &[ChatMessage],
    opts: &GenOpts,
) -> Result<Value, String> {
    let model = model.trim();
    if model.is_empty() {
        return Err("No model selected. Open Settings, choose a model, then retry.".into());
    }
    if provider == "ollama" {
        Ok(json!({ "model": model, "messages": messages, "stream": true }))
    } else {
        Ok(json!({
            "model": model,
            "messages": messages,
            "stream": true,
            // Ask OpenAI-compatible providers to include usage in the stream
            // (`choices` deltas carry it when supported) instead of forcing
            // a second bookkeeping path.
            "stream_options": { "include_usage": true },
            "temperature": opts.temperature.unwrap_or(0.7),
            "max_tokens": opts.max_tokens.unwrap_or(2048),
        }))
    }
}

#[cfg(test)]
mod chat_payload_tests {
    use super::*;

    #[test]
    fn fallback_candidates_excludes_failed_dedupes_and_prioritizes_verified() {
        let catalog: Vec<String> = vec![
            "nvidia/nemotron-3-super-120b-a12b".into(),
            "openai/gpt-oss-20b".into(),
            "meta/llama-3.2-11b-vision-instruct".into(),
        ];
        let c = fallback_candidates(
            Some("openai/gpt-oss-20b"),
            &catalog,
            "nvidia/nemotron-3-super-120b-a12b",
        );
        assert_eq!(c[0], "openai/gpt-oss-20b");
        assert!(!c.contains(&"nvidia/nemotron-3-super-120b-a12b".to_string()));
        assert_eq!(c.len(), c.iter().collect::<std::collections::HashSet<_>>().len());
    }

    #[test]
    fn fallback_candidates_without_verified_uses_catalog_order() {
        let catalog: Vec<String> = vec!["a/a".into(), "b/b".into(), "c/c".into()];
        let c = fallback_candidates(None, &catalog, "z/z");
        assert_eq!(c, catalog);
    }

    #[test]
    fn fallback_candidates_skips_blank_entries() {
        let catalog: Vec<String> = vec!["".into(), "  ".into(), "m/m".into()];
        let c = fallback_candidates(Some("  ".into()), &catalog, "m/m");
        assert!(c.is_empty());
    }

    #[test]
    fn gated_models_parse_ignores_blank_lines() {
        let raw = Some("  nvidia/a  \n\n nvidia/b \r\n".to_string());
        assert_eq!(parse_gated_models(raw), vec!["nvidia/a", "nvidia/b"]);
        assert!(parse_gated_models(None).is_empty());
        assert!(parse_gated_models(Some("\n  \n".into())).is_empty());
    }

    #[test]
    fn preferred_start_model_skips_only_models_already_gated() {
        let gated = vec!["nvidia/nemotron-3-super-120b-a12b".to_string()];
        assert_eq!(
            preferred_start_model("nvidia/nemotron-3-super-120b-a12b", Some("deepseek-ai/deepseek-v4.1-flash"), &gated),
            "deepseek-ai/deepseek-v4.1-flash"
        );
        // A model the key has not been refused for is honoured as chosen.
        assert_eq!(
            preferred_start_model("nvidia/some-new-model", Some("deepseek-ai/deepseek-v4.1-flash"), &gated),
            "nvidia/some-new-model"
        );
        // Nothing verified yet: keep the requested model so the gate can be re-checked.
        assert_eq!(
            preferred_start_model("nvidia/nemotron-3-super-120b-a12b", None, &gated),
            "nvidia/nemotron-3-super-120b-a12b"
        );
        // Verified equal to the gated request is not a swap target.
        assert_eq!(
            preferred_start_model("  nvidia/nemotron-3-super-120b-a12b  ", Some("nvidia/nemotron-3-super-120b-a12b"), &gated),
            "nvidia/nemotron-3-super-120b-a12b"
        );
    }

    /// Live sanity check of the exact production fallback path for a
    /// tier-gated model. Skipped by default because it hits the network
    /// with the saved NVIDIA key: run once with `cargo test -- --ignored`.
    #[test]
    #[ignore = "live API check: needs a saved NVIDIA key and network access"]
    fn live_find_served_model_recovers_from_gated_model() {
        let gated = "nvidia/nemotron-3-super-120b-a12b";
        let key = get_key_sync("nvidia")
            .unwrap()
            .expect("NVIDIA key must be saved in the credential store");
        let mut db_path = std::env::temp_dir();
        db_path.push("cameo_livetest.tmp.db");
        let _ = std::fs::remove_file(&db_path);
        let state = AppState::new(crate::commands::store::open_db(&db_path));
        let client = reqwest::Client::new();
        let served = tauri::async_runtime::block_on(find_served_model(
            &client,
            "nvidia",
            "https://integrate.api.nvidia.com/v1",
            Some(&key),
            gated,
            &state,
        ))
        .expect("fallback must find at least one model served for this account");
        assert_ne!(served, gated);
        assert_eq!(
            prefs_get(&state, &verified_model_pref_key("nvidia")).as_deref(),
            Some(served.as_str()),
            "verified model must be persisted"
        );
        let _ = std::fs::remove_file(&db_path);
    }

    /// Live model-serving survey: reports which providers have keys, which
    /// NVIDIA models this key can actually serve (strongest first), and
    /// proves one end-to-end chat round-trip on the winner. Skipped by
    /// default: hits the network with saved keys — run with
    /// `cargo test -- --ignored live_best_serving_model --nocapture`.
    /// Prints only model IDs and statuses, never keys.
    #[test]
    #[ignore = "live API check: needs saved keys and network access"]
    fn live_best_serving_model() {
        for provider in ["openrouter", "nvidia", "compatible"] {
            let has = get_key_sync(provider).ok().flatten().is_some();
            println!("KEY {provider}: {}", if has { "present" } else { "missing" });
        }
        let ollama_up = tauri::async_runtime::block_on(async {
            reqwest::Client::new()
                .get("http://localhost:11434/api/tags")
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false)
        });
        println!("OLLAMA: {}", if ollama_up { "reachable" } else { "down" });
        let key = get_key_sync("nvidia")
            .ok()
            .flatten()
            .expect("NVIDIA key must be saved to survey serving models");
        let base = provider_base("nvidia", &GenOpts::default()).expect("nvidia base");
        // Generous timeout: a cold NIM model can hold the connection 60s+
        // while it loads. Anything shorter misreports serving models as dead.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(90))
            .build()
            .expect("survey client");
        let catalog: Vec<String> = tauri::async_runtime::block_on(async {
            let res = client
                .get(format!("{base}/models"))
                .bearer_auth(&key)
                .send()
                .await;
            match res {
                Ok(r) if r.status().as_u16() == 200 => r
                    .json::<Value>()
                    .await
                    .ok()
                    .and_then(|v| {
                        v["data"].as_array().map(|items| {
                            items
                                .iter()
                                .filter_map(|m| m["id"].as_str().map(str::to_string))
                                .collect()
                        })
                    })
                    .unwrap_or_default(),
                _ => Vec::new(),
            }
        });
        println!("CATALOG: {} models", catalog.len());
        for m in &catalog {
            println!("MODEL {m}");
        }
        // Strongest reasoning first; only IDs actually listed for this account are probed.
        const PREFERRED: [&str; 13] = [
            "deepseek-ai/deepseek-r1",
            "deepseek-ai/deepseek-v4.1-flash",
            "moonshotai/kimi-k2.6",
            "z-ai/glm-5.3",
            "meta/llama-3.2-90b-vision-instruct",
            "mistralai/mistral-large-2-instruct",
            "nvidia/llama-3.1-nemotron-70b-instruct",
            "writer/palmyra-creative-122b",
            "stepfun-ai/step-5-preview",
            "nvidia/llama-3.1-nemotron-ultra-253b-v1",
            "z-ai/glm-5.3-flash",
            "openai/gpt-oss-20b",
            "mistralai/mistral-7b-instruct-v0.3",
        ];
        let mut winner: Option<String> = None;
        for cand in PREFERRED.iter().filter(|c| catalog.iter().any(|m| m == **c)) {
            // Inline probe with the failure reason (status vs transport) so a
            // dead network is never mistaken for a tier gate.
            let detail: String = tauri::async_runtime::block_on(async {
                let body = json!({
                    "model": cand,
                    "messages": [{ "role": "user", "content": "ping" }],
                    "max_tokens": 8,
                    "temperature": 0,
                    "stream": false,
                });
                match client
                    .post(format!("{base}/chat/completions"))
                    .bearer_auth(&key)
                    .json(&body)
                    .send()
                    .await
                {
                    Ok(r) => format!("http {}", r.status().as_u16()),
                    Err(e) => format!("transport: {:#}", e),
                }
            });
            let serves = detail == "http 200";
            println!("PROBE {cand}: {detail}");
            if serves {
                winner = Some(cand.to_string());
                break;
            }
        }
        let winner = winner.expect("no preferred model served; pick one from CATALOG/models");
        println!("WINNER: {winner}");
        // End-to-end proof: one real chat round-trip on the winner. The raw
        // shape is printed because some NIM models answer in `reasoning_content`
        // or part-list content instead of a plain content string.
        let started = std::time::Instant::now();
        let (reply, raw) = tauri::async_runtime::block_on(async {
            let body = json!({
                "model": winner,
                "messages": [{ "role": "user", "content": "Say hello in five words or less." }],
                "max_tokens": 64,
                "temperature": 0,
                "stream": false,
            });
            let r = client
                .post(format!("{base}/chat/completions"))
                .bearer_auth(&key)
                .json(&body)
                .send()
                .await
                .expect("chat ping send");
            assert_eq!(r.status().as_u16(), 200, "chat ping status");
            let v: Value = r.json().await.expect("chat ping json");
            let msg = &v["choices"][0]["message"];
            let mut text = msg["content"].as_str().unwrap_or("").to_string();
            if text.is_empty() {
                if let Some(parts) = msg["content"].as_array() {
                    text = parts
                        .iter()
                        .filter_map(|p| p["text"].as_str().or_else(|| p.as_str()))
                        .collect::<Vec<_>>()
                        .join("");
                }
            }
            if text.is_empty() {
                for k in ["reasoning_content", "reasoning"] {
                    if let Some(s) = msg.get(k).and_then(Value::as_str) {
                        if !s.trim().is_empty() {
                            text = format!("[{k}] {s}");
                            break;
                        }
                    }
                }
            }
            let raw = v.to_string();
            let raw = if raw.len() > 600 { format!("{}…", &raw[..600]) } else { raw };
            (text, raw)
        });
        println!("PING: {reply:?} in {:.1}s", started.elapsed().as_secs_f32());
        println!("RAW: {raw}");
        assert!(!reply.trim().is_empty(), "winner must produce answer text");
        // Every agent serves: parse the REAL system prompts out of the
        // frontend roster (same `"..." + "..."` literal chains the UI
        // sends) and get a live answer for each one on the winner model.
        const AGENT_IDS: [&str; 8] = [
            "cameo", "atlas", "forge", "pixel", "sentinel", "counsel", "medic", "ledger",
        ];
        let roster = include_str!("../../../src/types/tasks.ts");
        for id in AGENT_IDS {
            let prompt = extract_ts_prompt(roster, id)
                .unwrap_or_else(|| panic!("agent '{id}' must have a parseable prompt in tasks.ts"));
            assert!(prompt.len() > 40, "agent '{id}' prompt must be non-trivial");
            let started = std::time::Instant::now();
            let answer = tauri::async_runtime::block_on(ping_with_retry(
                &client,
                &base,
                &key,
                &winner,
                &prompt,
                "Reply with the single word READY and nothing else.",
                256,
            ))
            .unwrap_or_else(|e| panic!("agent '{id}' ping failed: {e}"));
            let preview: String = answer.chars().take(60).collect();
            println!("AGENT {id}: {preview:?} in {:.1}s", started.elapsed().as_secs_f32());
            assert!(!answer.trim().is_empty(), "agent '{id}' must get an answer");
            std::thread::sleep(Duration::from_secs(5));
        }
        println!("ALL AGENTS SERVE on {winner}");
    }

    /// Parse a `"lit" + "lit" + ...` chain; returns the joined text and the
    /// remainder after the chain.
    fn parse_chain(mut rest: &str) -> Option<(String, &str)> {
        let mut out = String::new();
        loop {
            // A non-literal continuation (e.g. the ADVISORY_BOUNDARY
            // identifier) ends the chain; keep what was parsed so far.
            if !rest.trim_start().starts_with('"') {
                break;
            }
            rest = rest.trim_start();
            let body = &rest[1..];
            let mut lit = String::new();
            let mut end: Option<usize> = None;
            let mut esc = false;
            for (i, c) in body.char_indices() {
                if esc {
                    lit.push(match c {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == '"' {
                    end = Some(i);
                    break;
                } else {
                    lit.push(c);
                }
            }
            out.push_str(&lit);
            rest = body[end? + 1..].trim_start();
            if let Some(after_plus) = rest.strip_prefix('+') {
                rest = after_plus;
            } else {
                break;
            }
        }
        if out.is_empty() {
            None
        } else {
            Some((out, rest))
        }
    }

    /// Pull one agent's system prompt out of `src/types/tasks.ts`, following
    /// the `"literal" + "literal"` chains the roster is written in.
    /// Advisory specialists append the shared ADVISORY_BOUNDARY constant,
    /// which is resolved from its definition in the same file.
    fn extract_ts_prompt(src: &str, id: &str) -> Option<String> {
        let start = src.find(&format!("id: \"{id}\""))?;
        let mut rest = &src[start..];
        rest = &rest[rest.find("prompt:")? + "prompt:".len()..];
        let (mut out, mut rest) = parse_chain(rest)?;
        rest = rest.trim_start();
        if rest.starts_with("ADVISORY_BOUNDARY") {
            let def_at = src.find("ADVISORY_BOUNDARY =")?;
            let def_rest = &src[def_at + "ADVISORY_BOUNDARY =".len()..];
            let (boundary, _) = parse_chain(def_rest)?;
            out.push_str(&boundary);
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    /// Answer text however the model packaged it: plain content, part-list
    /// content, or a reasoning trace (some NIM models answer there).
    fn answer_text(msg: &Value) -> String {
        if let Some(s) = msg["content"].as_str() {
            if !s.trim().is_empty() {
                return s.to_string();
            }
        }
        if let Some(parts) = msg["content"].as_array() {
            let joined = parts
                .iter()
                .filter_map(|p| p["text"].as_str().or_else(|| p.as_str()))
                .collect::<Vec<_>>()
                .join("");
            if !joined.trim().is_empty() {
                return joined;
            }
        }
        for k in ["reasoning_content", "reasoning"] {
            if let Some(s) = msg.get(k).and_then(Value::as_str) {
                if !s.trim().is_empty() {
                    return s.to_string();
                }
            }
        }
        String::new()
    }

    /// One chat round-trip with retries: free-tier NIM answers 429s and
    /// mid-load transport resets while a model is still warming up.
    async fn ping_with_retry(
        client: &reqwest::Client,
        base: &str,
        key: &str,
        model: &str,
        system: &str,
        user: &str,
        max_tokens: u64,
    ) -> Result<String, String> {
        let body = json!({
            "model": model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
            "max_tokens": max_tokens,
            "temperature": 0,
            "stream": false,
        });
        let mut last = "no attempts".to_string();
        for attempt in 1..=3u32 {
            match client
                .post(format!("{base}/chat/completions"))
                .bearer_auth(key)
                .json(&body)
                .send()
                .await
            {
                Ok(r) if r.status().as_u16() == 200 => {
                    let v: Value = r.json().await.map_err(|e| format!("ping json: {e}"))?;
                    return Ok(answer_text(&v["choices"][0]["message"]));
                }
                Ok(r) => last = format!("http {}", r.status().as_u16()),
                Err(e) => last = format!("transport: {:#}", e),
            }
            eprintln!("ping attempt {attempt} ({model}): {last}; backing off");
            std::thread::sleep(Duration::from_secs(15));
        }
        Err(last)
    }

    /// Focused re-check for the advisory specialists (counsel/medic/ledger)
    /// against the surveyed winner, without re-running the full probe sweep.
    /// Run with `cargo test -- --ignored live_agents_counsel_medic_ledger --nocapture`.
    #[test]
    #[ignore = "live API check: needs the saved NVIDIA key and network access"]
    fn live_agents_counsel_medic_ledger() {
        let key = get_key_sync("nvidia")
            .ok()
            .flatten()
            .expect("NVIDIA key must be saved");
        let base = provider_base("nvidia", &GenOpts::default()).expect("nvidia base");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("survey client");
        let roster = include_str!("../../../src/types/tasks.ts");
        for id in ["counsel", "medic", "ledger"] {
            let prompt = extract_ts_prompt(roster, id)
                .unwrap_or_else(|| panic!("agent '{id}' must have a parseable prompt"));
            assert!(prompt.len() > 40, "agent '{id}' prompt must be non-trivial");
            let started = std::time::Instant::now();
            let answer = tauri::async_runtime::block_on(ping_with_retry(
                &client,
                &base,
                &key,
                "stepfun-ai/step-5-preview",
                &prompt,
                "Reply with the single word READY and nothing else.",
                256,
            ))
            .unwrap_or_else(|e| panic!("agent '{id}' ping failed: {e}"));
            let preview: String = answer.chars().take(60).collect();
            println!("AGENT {id}: {preview:?} in {:.1}s", started.elapsed().as_secs_f32());
            assert!(!answer.trim().is_empty(), "agent '{id}' must get an answer");
            std::thread::sleep(Duration::from_secs(5));
        }
        println!("ADVISORY AGENTS SERVE on stepfun-ai/step-5-preview");
    }

    fn messages() -> Vec<ChatMessage> {
        vec![ChatMessage { role: "user".into(), content: "hello".into() }]
    }

    #[test]
    fn openai_compatible_payload_has_nonempty_model() {
        let body = build_chat_payload("openrouter", "  vendor/model-x  ", &messages(), &GenOpts::default()).unwrap();
        assert_eq!(body["model"], "vendor/model-x");
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"][0]["content"], "hello");
    }

    #[test]
    fn ollama_payload_has_nonempty_model() {
        let body = build_chat_payload("ollama", "llama3.2", &messages(), &GenOpts::default()).unwrap();
        assert_eq!(body["model"], "llama3.2");
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn blank_model_is_rejected_instead_of_sending_bad_request() {
        let result = build_chat_payload("openrouter", "  ", &messages(), &GenOpts::default());
        assert!(result.is_err());
    }

    #[test]
    fn request_timeout_is_interpreted_in_milliseconds_and_bounded() {
        let timeout = GenOpts {
            timeout_ms: Some(120_000),
            ..GenOpts::default()
        };
        assert_eq!(request_timeout(&timeout), Duration::from_secs(120));
        assert_eq!(
            request_timeout(&GenOpts::default()),
            Duration::from_secs(600)
        );
        let short = GenOpts {
            timeout_ms: Some(1),
            ..GenOpts::default()
        };
        assert_eq!(request_timeout(&short), Duration::from_secs(1));
        let long = GenOpts {
            timeout_ms: Some(u64::MAX),
            ..GenOpts::default()
        };
        assert_eq!(request_timeout(&long), Duration::from_secs(600));
    }

    #[test]
    fn agent_tool_definitions_cover_the_operator_surface() {
        let defs = agent_tool_definitions();
        let names: Vec<&str> = defs
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|d| d["function"]["name"].as_str())
            .collect();
        assert_eq!(names, vec!["exec_run", "net_fetch", "net_search", "device_launch"]);
    }

    #[test]
    fn agent_max_rounds_defaults_and_clamps() {
        assert_eq!(agent_max_rounds(&GenOpts::default()), 4);
        assert_eq!(agent_max_rounds(&GenOpts { agent_max_rounds: Some(0), ..GenOpts::default() }), 1);
        assert_eq!(agent_max_rounds(&GenOpts { agent_max_rounds: Some(99), ..GenOpts::default() }), 8);
    }

    #[test]
    fn self_correction_split_surfaces_admission_as_thinking() {
        let (note, final_text) =
            split_self_correction("I was wrong about the path — it is src/app.ts. Fixed below.");
        assert!(note.is_some());
        assert!(final_text.contains("src/app.ts"));
        let (none, plain) = split_self_correction("Here is the plan: 1. Read 2. Edit 3. Verify.");
        assert!(none.is_none());
        assert_eq!(plain, "Here is the plan: 1. Read 2. Edit 3. Verify.");
    }

    #[test]
    fn main_brain_fallback_prefers_deepseek_first() {
        // DeepSeek R1 is the default brain: the seed probe order must lead
        // with the strongest reasoning model, then its faster sibling.
        let seeds = ["deepseek-ai/deepseek-r1", "deepseek-ai/deepseek-v4.1-flash", "openai/gpt-oss-20b"];
        assert!(seeds[0].starts_with("deepseek-ai/"));
        assert_eq!(seeds[0], "deepseek-ai/deepseek-r1");
    }

    #[test]
    fn agent_tool_approval_gates_match_operator_policy() {
        let safe_exec = json!({ "command": "cargo", "args": ["test"] });
        let destructive = json!({ "command": "rm", "args": ["-rf", "dist"] });
        // Safe exec auto-runs only when the caller opted in; otherwise it waits.
        assert!(!agent_tool_requires_approval("exec_run", &safe_exec, true));
        assert!(agent_tool_requires_approval("exec_run", &safe_exec, false));
        // Destructive commands always wait, even in auto-safe mode.
        assert!(agent_tool_requires_approval("exec_run", &destructive, true));
        // Invalid args can never auto-run.
        assert!(agent_tool_requires_approval("exec_run", &json!({}), true));
        // Device control always waits.
        assert!(agent_tool_requires_approval("device_launch", &json!({ "target": "calc" }), true));
        // Unknown tools never auto-run.
        assert!(agent_tool_requires_approval("format_disk", &json!({}), true));
    }

    #[test]
    fn agent_tool_preview_stays_short_and_readable() {
        let preview = agent_tool_preview("exec_run", &json!({ "command": "cargo", "args": ["test", "--", "--nocapture"] }));
        assert_eq!(preview, "cargo test -- --nocapture");
        let long = "x".repeat(500);
        assert!(agent_tool_preview("net_fetch", &json!({ "url": long })).chars().count() <= 241);
    }

    #[test]
    fn agent_payload_requests_non_streaming_tool_calls() {
        let convo = vec![json!({ "role": "user", "content": "hi" })];
        let body = agent_payload("vendor/model-x", &convo, &GenOpts::default());
        assert_eq!(body["stream"], false);
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["tools"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn agent_turn_parses_text_and_tool_calls() {
        let raw = json!({
            "choices": [{
                "message": {
                    "content": "checking",
                    "tool_calls": [{
                        "id": "call_1",
                        "function": { "name": "net_search", "arguments": "{\"query\": \"rust\"}" }
                    }]
                }
            }]
        });
        let turn = parse_agent_turn(&raw).unwrap();
        assert_eq!(turn.text, "checking");
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].id, "call_1");
        assert_eq!(turn.tool_calls[0].args["query"], "rust");
    }

    #[test]
    fn catalog_probe_list_is_bounded_and_skips_failed() {
        let catalog: Vec<String> = (0..20).map(|i| format!("vendor/model-{i}")).collect();
        let list = catalog_probe_list(&catalog, "vendor/model-0", 5);
        assert_eq!(list.len(), 5);
        assert!(!list.contains(&"vendor/model-0".to_string()));
        assert_eq!(list[0], "vendor/model-1");
    }

    #[test]
    fn agent_turn_extracts_reasoning_trace_as_thinking() {
        let raw = json!({
            "choices": [{
                "message": {
                    "content": "answer",
                    "reasoning_content": "let me think",
                    "tool_calls": []
                }
            }]
        });
        let turn = parse_agent_turn(&raw).unwrap();
        assert_eq!(turn.thinking, "let me think");
        assert_eq!(turn.text, "answer");
    }

    #[test]
    fn agent_turn_keeps_bad_arguments_reportable() {
        let raw = json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_9",
                        "function": { "name": "exec_run", "arguments": "{oops" }
                    }]
                }
            }]
        });
        let turn = parse_agent_turn(&raw).unwrap();
        assert!(turn.text.is_empty());
        assert!(turn.tool_calls[0].args.get("_invalid_json").is_some());
    }
}

// ---------------------------------------------------------------------------
// Autonomous agent tool loop — the model may call approved operator actions
// itself (exec inside the project root, public-https fetch, Exa web search,
// device launch). Every tool runs through the SAME operator gates as the
// manual console: autonomy/root/net/device switches, destructive-command
// confirmation, and audit logging. Destructive or device actions ALWAYS wait
// for an explicit `chat_tool_decision`; safe reads may auto-run only when the
// caller opts in with `agent_auto_approve_safe`.
// ---------------------------------------------------------------------------

/// Hard bounds on tool rounds (each round is one extra model call).
fn agent_max_rounds(opts: &GenOpts) -> u32 {
    opts.agent_max_rounds.unwrap_or(4).clamp(1, 8)
}

fn agent_tool_definitions() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "exec_run",
                "description": "Run a program inside the selected project root. Destructive commands need user approval (the orchestrator asks); just call with the program and args.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Program name only, e.g. 'cargo' or 'python'. No shell operators." },
                        "args": { "type": "array", "items": { "type": "string" }, "description": "Arguments, at most 20." },
                        "cwd": { "type": ["string", "null"], "description": "Optional subdirectory inside the project root." }
                    },
                    "required": ["command"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "net_fetch",
                "description": "GET a public https URL (never localhost or private IPs). Returns status and body text.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "The https URL to fetch." }
                    },
                    "required": ["url"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "net_search",
                "description": "Web search via Exa. Needs internet access enabled and an Exa key saved in Settings.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query." },
                        "num_results": { "type": "integer", "minimum": 1, "maximum": 10, "description": "How many hits (default 5)." }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "device_launch",
                "description": "Hand a path or URL to the OS default handler. ALWAYS needs user approval.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "description": "App, file or URL to open." }
                    },
                    "required": ["target"],
                    "additionalProperties": false
                }
            }
        }
    ])
}

fn agent_system_prompt() -> &'static str {
    "You are CAMEO, an on-device orchestrator with a scoped operator layer: commands run inside one project root the user picked, network is public-https only, and launching apps always needs the user's approval (the orchestrator asks — just call the tool). Use tools when live facts or local execution genuinely help; otherwise answer directly. Keep arguments minimal and never invent tool results. Work in a disciplined loop: PLAN (brief numbered steps) → ACT (one minimal tool call per need) → VERIFY (read the tool result, check it actually satisfies the step) → REFLECT (if a result is an error, empty, or contradicts the goal, diagnose the cause, adjust arguments or strategy, and retry differently — never repeat the identical failing call) → FINALIZE (only answer when every step is verified or you can state exactly what failed and what the user should do). If you catch your own mistake mid-reasoning, say so and correct it before the final answer. Final answers must be concrete and actionable: what was done, what was found, what remains, and the exact next command or click. Never return an empty answer; if tools gave nothing usable, explain what you tried, what each attempt returned, and the smallest next step."
}

fn agent_payload(model: &str, convo: &[Value], opts: &GenOpts) -> Value {
    json!({
        "model": model,
        "messages": convo,
        "stream": false,
        "temperature": opts.temperature.unwrap_or(0.7),
        "max_tokens": opts.max_tokens.unwrap_or(2048),
        "tools": agent_tool_definitions(),
        "tool_choice": "auto",
    })
}

struct AgentToolCall {
    id: String,
    name: String,
    args: Value,
}

struct AgentTurn {
    text: String,
    thinking: String,
    tool_calls: Vec<AgentToolCall>,
}

/// Parse one non-streaming chat-completions response. Argument JSON that does
/// not parse is kept as `_invalid_json` so the loop can report it back to the
/// model as a tool error instead of executing something unintended.
fn parse_agent_turn(v: &Value) -> Result<AgentTurn, String> {
    let msg = v["choices"][0]["message"].as_object().ok_or_else(|| {
        let detail = truncate(&v.to_string(), 200);
        format!("Provider returned no message: {detail}")
    })?;
    let mut text = String::new();
    match msg.get("content") {
        Some(Value::String(s)) => text.push_str(s),
        Some(Value::Array(parts)) => {
            for p in parts {
                if let Some(s) = p["text"].as_str() {
                    text.push_str(s);
                } else if let Some(s) = p.as_str() {
                    text.push_str(s);
                }
            }
        }
        _ => {}
    }
    // Reasoning models (DeepSeek-R1, QwQ, OpenRouter reasoning variants)
    // return their trace alongside the content — surface it as thinking.
    let mut thinking = String::new();
    for key in ["reasoning_content", "reasoning", "thought"] {
        if let Some(s) = msg.get(key).and_then(Value::as_str) {
            if !s.trim().is_empty() {
                thinking.push_str(s);
                break;
            }
        }
    }
    let mut tool_calls = Vec::new();
    if let Some(items) = msg.get("tool_calls").and_then(Value::as_array) {
        for (i, tc) in items.iter().enumerate() {
            let id = tc["id"]
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("call_{i}"));
            let name = tc["function"]["name"].as_str().unwrap_or("unknown").to_string();
            let args = match tc["function"].get("arguments") {
                Some(Value::Object(_)) => tc["function"]["arguments"].clone(),
                Some(Value::String(s)) if s.trim().is_empty() => json!({}),
                Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| {
                    json!({ "_invalid_json": truncate(s, 500) })
                }),
                _ => json!({}),
            };
            tool_calls.push(AgentToolCall { id, name, args });
        }
    }
    Ok(AgentTurn { text, thinking, tool_calls })
}

/// Split a "learned mistake" admission from the final answer so the UI can
/// highlight the correction live in the thinking stream. Returns
/// (admission_note, final_text). Pure helper, unit-tested.
fn split_self_correction(text: &str) -> (Option<String>, String) {
    let lower = text.to_lowercase();
    let markers = [
        "i was wrong", "my mistake", "correction:", "i misread", "actually,",
        "on reflection", "let me correct", "to correct myself",
    ];
    let mut hit: Option<usize> = None;
    for m in markers {
        if let Some(pos) = lower.find(m) {
            hit = Some(match hit {
                Some(prev) => prev.min(pos),
                None => pos,
            });
        }
    }
    match hit {
        Some(pos) => {
            // Byte index from a lowercase search is safe to slice at only on
            // a char boundary; fall back to the full text otherwise.
            let before = text.get(..pos).unwrap_or("").trim();
            let rest = text.get(pos..).unwrap_or(text).trim();
            if rest.is_empty() {
                (None, text.to_string())
            } else if before.is_empty() {
                // Admission opens the reply: surface its first sentence.
                let end = rest
                    .find(['.', '!', '?', '\n'])
                    .map(|i| (i + 1).min(rest.len()))
                    .unwrap_or(rest.len().min(140));
                let first = rest.get(..end).unwrap_or(rest).trim();
                (Some(format!("Self-correction: {first}")), text.to_string())
            } else {
                (Some(format!("Self-correction: {rest}")), text.to_string())
            }
        }
        None => (None, text.to_string()),
    }
}

fn truncate_output_chars(s: String, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= n {
        s
    } else {
        format!("{}…", chars[..n].iter().collect::<String>())
    }
}

fn agent_tool_preview(name: &str, args: &Value) -> String {
    let raw = match name {
        "exec_run" => {
            let cmd = args["command"].as_str().unwrap_or("?");
            let rest = args["args"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "))
                .unwrap_or_default();
            format!("{cmd} {rest}").trim_end().to_string()
        }
        "net_fetch" => args["url"].as_str().unwrap_or("?").to_string(),
        "net_search" => args["query"].as_str().unwrap_or("?").to_string(),
        "device_launch" => args["target"].as_str().unwrap_or("?").to_string(),
        _ => args.to_string(),
    };
    truncate_output_chars(raw, 240)
}

fn is_known_agent_tool(name: &str) -> bool {
    matches!(name, "exec_run" | "net_fetch" | "net_search" | "device_launch")
}

/// Same validation the operator console enforces, evaluated BEFORE anything
/// runs so the UI can show whether a click is needed.
fn agent_tool_requires_approval(name: &str, args: &Value, auto_safe: bool) -> bool {
    match name {
        // Device control always needs an explicit click.
        "device_launch" => true,
        "exec_run" => {
            if !auto_safe {
                return true;
            }
            let valid = operator::validate_exec_args(args).map(|v| (v.command, v.args));
            match valid {
                Ok((command, argv)) => operator::destructive_reason(&command, &argv).is_some(),
                Err(_) => true,
            }
        }
        "net_fetch" => !auto_safe || operator::validate_fetch_args(args).is_err(),
        "net_search" => !auto_safe || operator::validate_search_args(args).is_err(),
        _ => true,
    }
}

struct AgentHttp {
    provider: String,
    client: reqwest::Client,
    base: String,
    key: Option<String>,
    auto_proxy_used: bool,
    timeout: Duration,
}

fn agent_http(provider: &str, opts: &GenOpts, state: &AppState) -> Result<AgentHttp, String> {
    let base = provider_base(provider, opts)?;
    let proxy_explicit = opts.proxy_profile_id.is_some();
    let proxy_eff: Option<String> = opts.proxy_profile_id.clone().or_else(|| active_proxy_id(state));
    let auto_proxy_used = proxy_eff.is_some() && !proxy_explicit;
    let timeout = request_timeout(opts);
    let client = cached_client(proxy_eff.as_deref(), state, timeout)?;
    let key = match get_key_sync(provider)? {
        Some(k) => Some(k),
        None => return Err(format!("No API key saved for {provider}. Add one in Settings.")),
    };
    Ok(AgentHttp { provider: provider.to_string(), client, base, key, auto_proxy_used, timeout })
}

/// One non-streaming POST with the same best-effort proxy fallback as streamed chat.
async fn agent_post_raw(state: &AppState, ctx: &mut AgentHttp, body: &Value) -> Result<reqwest::Response, String> {
    let send = |client: &reqwest::Client| {
        let mut r = client.post(format!("{}/chat/completions", ctx.base)).json(body);
        if let Some(k) = &ctx.key {
            r = r.bearer_auth(k);
        }
        if ctx.provider == "openrouter" {
            r = r.header("HTTP-Referer", "https://cameo.local");
        }
        r.send()
    };
    match send(&ctx.client).await {
        Ok(r) => Ok(r),
        Err(_) if ctx.auto_proxy_used => {
            ctx.client = cached_client(None, state, ctx.timeout)?;
            ctx.auto_proxy_used = false;
            send(&ctx.client).await.map_err(|e| format!("network error calling {}: {e}", ctx.provider))
        }
        Err(e) => Err(format!("network error calling {}: {e}", ctx.provider)),
    }
}

/// POST one agent round, honouring the NVIDIA tier-gate fallback (skip a model
/// the key was already refused for, probe for a served one, remember it).
async fn agent_round_post(
    state: &AppState,
    ctx: &mut AgentHttp,
    model: &mut String,
    convo: &[Value],
    opts: &GenOpts,
) -> Result<Value, String> {
    for _ in 0..5 {
        let body = agent_payload(model, convo, opts);
        let resp = agent_post_raw(state, ctx, &body).await?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if status == 200 {
            persist_verified_model(state, &ctx.provider, model);
            return serde_json::from_str::<Value>(&text)
                .map_err(|e| format!("bad agent response from {}: {e}", ctx.provider));
        }
        let tier_gated = status == 404 && ctx.provider == "nvidia" && text.contains("Not found for account");
        if tier_gated {
            if let Some(fallback) = find_served_model(
                &ctx.client,
                &ctx.provider,
                &ctx.base,
                ctx.key.as_deref(),
                model,
                state,
            )
            .await
            {
                remember_gated_model(state, &ctx.provider, model);
                persist_model_selection(state, &ctx.provider, &fallback);
                persist_verified_model(state, &ctx.provider, &fallback);
                *model = fallback;
                continue;
            }
        }
        return Err(http_error(status, &text));
    }
    Err(format!("Tried 5 fallback models for '{}' and none answered.", ctx.provider))
}

/// Wait (blocking-friendly polling) for the user to answer a tool approval.
/// Returns Ok(true) = approved, Ok(false) = denied or timed out.
fn await_tool_decision(
    state: &AppState,
    approval_id: &str,
    cancelled: &Arc<AtomicBool>,
) -> Result<bool, String> {
    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    {
        let mut map = state.tool_approvals.lock().map_err(|e| e.to_string())?;
        map.insert(approval_id.to_string(), tx);
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(600);
    loop {
        if cancelled.load(Ordering::SeqCst) {
            let mut map = state.tool_approvals.lock().map_err(|e| e.to_string())?;
            map.remove(approval_id);
            return Err("Cancelled".into());
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(decision) => {
                let mut map = state.tool_approvals.lock().map_err(|e| e.to_string())?;
                map.remove(approval_id);
                return Ok(decision);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let mut map = state.tool_approvals.lock().map_err(|e| e.to_string())?;
                map.remove(approval_id);
                return Ok(false);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if std::time::Instant::now() >= deadline {
                    let mut map = state.tool_approvals.lock().map_err(|e| e.to_string())?;
                    map.remove(approval_id);
                    return Ok(false);
                }
            }
        }
    }
}

/// The frontend's answer to a pending approval shown in chat/operator UI.
#[tauri::command]
pub async fn chat_tool_decision(
    approval_id: String,
    approved: bool,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let sender = {
        let mut map = state.tool_approvals.lock().map_err(|e| e.to_string())?;
        map.remove(&approval_id)
    };
    match sender {
        Some(tx) => tx.send(approved).map(|_| true).map_err(|_| "approval already resolved".to_string()),
        None => Ok(false),
    }
}

/// Run one approved tool through the real operator implementation, so every
/// gate (autonomy, root, net, device, destructive confirmation, audit) holds.
async fn execute_agent_tool(state: &AppState, name: &str, args: &Value) -> (bool, String) {
    let out: Result<Value, String> = match name {
        "exec_run" => match operator::validate_exec_args(args) {
            Ok(v) => operator::exec_run_impl(state, &v.command, &v.args, v.cwd.as_deref(), true).await,
            Err(e) => Err(e),
        },
        "net_fetch" => match operator::validate_fetch_args(args) {
            Ok(url) => operator::net_fetch_impl(state, &url).await,
            Err(e) => Err(e),
        },
        "net_search" => match operator::validate_search_args(args) {
            Ok(v) => operator::net_search_impl(state, &v.query, v.count).await,
            Err(e) => Err(e),
        },
        "device_launch" => match operator::validate_device_args(args) {
            Ok(target) => operator::device_launch_impl(state, &target),
            Err(e) => Err(e),
        },
        other => Err(format!("unknown tool '{other}'")),
    };
    match out {
        Ok(v) => (true, truncate_output_chars(json_to_tool_text(&v), 6000)),
        Err(e) => (false, truncate_output_chars(e, 6000)),
    }
}

fn json_to_tool_text(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.to_string();
    }
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

#[allow(clippy::too_many_arguments)]
async fn agent_tool_loop(
    request_id: &str,
    provider: &str,
    model: &str,
    messages: Vec<ChatMessage>,
    opts: GenOpts,
    on_event: Channel<ChatEvent>,
    state: &AppState,
    cancelled: &Arc<AtomicBool>,
) -> Result<(), String> {
    let mut ctx = match agent_http(provider, &opts, state) {
        Ok(ctx) => ctx,
        Err(message) => {
            let _ = on_event.send(ChatEvent::Error { message });
            return Ok(());
        }
    };
    let auto_safe = opts.agent_auto_approve_safe.unwrap_or(false);
    let auto_all = opts.agent_auto_approve_all.unwrap_or(false);
    let max_rounds = agent_max_rounds(&opts);
    let mut current_model = preferred_start_model(
        model,
        prefs_get(state, &verified_model_pref_key(provider)).as_deref(),
        &gated_models(state, provider),
    );
    if current_model != model.trim() {
        let _ = on_event.send(ChatEvent::Delta {
            text: format!(
                "Using '{current_model}' — '{}' is not enabled for this key.\n\n",
                model.trim()
            ),
        });
    }

    let mut convo: Vec<Value> = Vec::with_capacity(messages.len() + 8);
    convo.push(json!({ "role": "system", "content": agent_system_prompt() }));
    for m in &messages {
        convo.push(json!({ "role": m.role, "content": m.content }));
    }
    let mut prompt_tokens: u64 = 0;
    let mut completion_tokens: u64 = 0;
    // Live self-correction bookkeeping: consecutive failed tool results.
    // Two in a row triggers an explicit reflection nudge so the model
    // diagnoses and changes strategy instead of repeating the same call.
    let mut consecutive_failures: u32 = 0;

    for round in 0..max_rounds {
        if cancelled.load(Ordering::SeqCst) {
            let _ = on_event.send(ChatEvent::Error { message: "Cancelled".into() });
            return Ok(());
        }            let phase = if round == 0 {
                format!("Contacting {provider} ({current_model})...")
            } else {
                format!("Planning follow-up (round {} of {max_rounds})...", round + 1)
            };
            let _ = on_event.send(ChatEvent::Status { message: phase });
        let raw = match agent_round_post(state, &mut ctx, &mut current_model, &convo, &opts).await {
            Ok(v) => v,
            Err(message) => {
                let _ = on_event.send(ChatEvent::Error { message });
                return Ok(());
            }
        };
        if let Some(u) = raw["usage"].as_object() {
            prompt_tokens += u.get("prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
            completion_tokens += u.get("completion_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
        }
        let turn = match parse_agent_turn(&raw) {
            Ok(t) => t,
            Err(message) => {
                let _ = on_event.send(ChatEvent::Error { message });
                return Ok(());
            }
        };
        if !turn.thinking.is_empty() {
            let _ = on_event.send(ChatEvent::Thinking { text: turn.thinking });
        }
        if turn.tool_calls.is_empty() {
            if !turn.text.is_empty() {
                // Treat a learned-mistake admission as progress, not failure:
                // surface it in the thinking stream so the UI highlights the
                // correction live, then deliver the corrected final text.
                let (admission, final_text) = split_self_correction(&turn.text);
                if let Some(note) = admission {
                    let _ = on_event.send(ChatEvent::Thinking { text: note });
                }
                let _ = on_event.send(ChatEvent::Delta { text: final_text });
            } else if round + 1 < max_rounds {
                // Empty reply with rounds left: don't give up — ask the
                // model once to repair it. Visible live in the todo stream.
                let _ = on_event.send(ChatEvent::Status {
                    message: "Empty reply — asking the model to repair it…".into(),
                });
                convo.push(json!({ "role": "assistant", "content": turn.text }));
                convo.push(json!({
                    "role": "user",
                    "content": "Your last reply was empty. Recover: either answer the original request directly with a concrete, actionable response, or make exactly one tool call that moves it forward. Never reply empty."
                }));
                continue;
            } else {
                let _ = on_event.send(ChatEvent::Delta {
                    text: "(empty response — the model returned no content)".into(),
                });
            }
            if prompt_tokens > 0 || completion_tokens > 0 {
                let _ = on_event.send(ChatEvent::Usage { prompt_tokens, completion_tokens });
            }
            let _ = on_event.send(ChatEvent::Done);
            return Ok(());
        }

        // Record the assistant turn (with its tool calls) so results attach to it.
        let calls_json: Vec<Value> = turn
            .tool_calls
            .iter()
            .map(|c| json!({ "id": c.id, "type": "function", "function": { "name": c.name, "arguments": c.args.to_string() } }))
            .collect();
        convo.push(json!({ "role": "assistant", "content": turn.text, "tool_calls": calls_json }));

        for (index, call) in turn.tool_calls.iter().enumerate() {
            if cancelled.load(Ordering::SeqCst) {
                let _ = on_event.send(ChatEvent::Error { message: "Cancelled".into() });
                return Ok(());
            }
            let approval_id = format!("{request_id}-t{round}-{index}");
            // Autopilot skips every approval gate (still audit-logged);
            // otherwise the per-tool operator policy decides.
            let requires_approval = !auto_all
                && is_known_agent_tool(&call.name)
                && agent_tool_requires_approval(&call.name, &call.args, auto_safe);
            let preview = agent_tool_preview(&call.name, &call.args);
            let _ = on_event.send(ChatEvent::Tool {
                approval_id: approval_id.clone(),
                call_id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.args.clone(),
                preview: preview.clone(),
                requires_approval,
            });

            let approved = if !is_known_agent_tool(&call.name) {
                false
            } else if !requires_approval {
                operator::audit_agent_action(
                    state,
                    "tool",
                    "auto_approved",
                    json!({ "approvalId": approval_id, "tool": call.name, "preview": preview }),
                );
                true
            } else {
                operator::audit_agent_action(
                    state,
                    "tool",
                    "approval_requested",
                    json!({ "approvalId": approval_id, "tool": call.name, "preview": preview }),
                );
                match await_tool_decision(state, &approval_id, cancelled) {
                    Ok(decision) => {
                        operator::audit_agent_action(
                            state,
                            "tool",
                            if decision { "approved" } else { "denied" },
                            json!({ "approvalId": approval_id, "tool": call.name }),
                        );
                        let _ = on_event.send(ChatEvent::ToolApproval { approval_id: approval_id.clone(), approved: decision });
                        decision
                    }
                    Err(message) => {
                        let _ = on_event.send(ChatEvent::Error { message });
                        return Ok(());
                    }
                }
            };

            if approved {
                let _ = on_event.send(ChatEvent::Status {
                    message: format!("Running {}…", call.name),
                });
            }
            let (ok, output) = if approved {
                execute_agent_tool(state, &call.name, &call.args).await
            } else if !is_known_agent_tool(&call.name) {
                (false, format!("unknown tool '{}' — available: exec_run, net_fetch, net_search, device_launch", call.name))
            } else {
                operator::audit_agent_action(
                    state,
                    "tool",
                    "denied",
                    json!({ "approvalId": approval_id, "tool": call.name }),
                );
                (false, "The user denied this tool call. Do not retry it; work another way or explain what you need.".to_string())
            };
            let _ = on_event.send(ChatEvent::ToolResult {
                approval_id: approval_id.clone(),
                call_id: call.id.clone(),
                name: call.name.clone(),
                ok,
                output: output.clone(),
            });
            // Live error detection: track failures and force a strategy
            // change before the next round instead of a blind retry.
            if ok {
                consecutive_failures = 0;
            } else {
                consecutive_failures += 1;
                let _ = on_event.send(ChatEvent::Status {
                    message: format!(
                        "{} failed — diagnosing before retry ({}/{})…",
                        call.name, consecutive_failures, 2
                    ),
                });
            }
            convo.push(json!({ "role": "tool", "tool_call_id": call.id, "content": output }));
            if !ok && consecutive_failures >= 2 {
                convo.push(json!({
                    "role": "user",
                    "content": "Two consecutive tool results failed. Stop and reflect: (1) quote the exact error, (2) name the single most likely cause, (3) state the one different approach you will try next (different arguments, different tool, or answering from what you already know). Do NOT repeat an identical failing call."
                }));
                consecutive_failures = 0;
            }
        }
    }
    // Rounds exhausted with tool results but no final answer: make ONE
    // bounded synthesis pass so the user gets a usable final response
    // instead of a "stopped" stub. The synthesis prompt forces grounding
    // in the tool results already collected.
    let _ = on_event.send(ChatEvent::Status {
        message: "Synthesizing a final answer from the collected results…".into(),
    });
    convo.push(json!({
        "role": "user",
        "content": "Tool rounds are exhausted. Write the final response NOW, grounded only in the tool results above: (1) what was done, (2) the key findings with file paths/URLs where relevant, (3) what failed if anything and the exact next step. Do not call more tools. Do not invent results. Keep it actionable."
    }));
    match agent_round_post(state, &mut ctx, &mut current_model, &convo, &opts).await {
        Ok(v) => {
            if let Some(u) = v["usage"].as_object() {
                prompt_tokens += u.get("prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                completion_tokens += u.get("completion_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
            }
            match parse_agent_turn(&v) {
                Ok(turn) => {
                    if !turn.thinking.is_empty() {
                        let _ = on_event.send(ChatEvent::Thinking { text: turn.thinking });
                    }
                    if !turn.text.is_empty() {
                        let (admission, final_text) = split_self_correction(&turn.text);
                        if let Some(note) = admission {
                            let _ = on_event.send(ChatEvent::Thinking { text: note });
                        }
                        let _ = on_event.send(ChatEvent::Delta { text: final_text });
                    } else {
                        let _ = on_event.send(ChatEvent::Delta {
                            text: format!("Stopped after {max_rounds} tool rounds without a final answer — ask me to continue or narrow the task."),
                        });
                    }
                }
                Err(_) => {
                    let _ = on_event.send(ChatEvent::Delta {
                        text: format!("Stopped after {max_rounds} tool rounds without a final answer — ask me to continue or narrow the task."),
                    });
                }
            }
        }
        Err(_) => {
            let _ = on_event.send(ChatEvent::Delta {
                text: format!("Stopped after {max_rounds} tool rounds without a final answer — ask me to continue or narrow the task."),
            });
        }
    }
    if prompt_tokens > 0 || completion_tokens > 0 {
        let _ = on_event.send(ChatEvent::Usage { prompt_tokens, completion_tokens });
    }
    let _ = on_event.send(ChatEvent::Done);
    Ok(())
}

#[tauri::command]
pub async fn chat(
    request_id: String,
    provider: String,
    model: String,
    messages: Vec<ChatMessage>,
    opts: GenOpts,
    on_event: Channel<ChatEvent>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let flag = Arc::new(AtomicBool::new(false));
    {
        let mut map = state.cancel_flags.lock().map_err(|e| e.to_string())?;
        map.insert(request_id.clone(), flag.clone());
    }
    let state_inner = state.inner().clone();
    let req_id = request_id.clone();
    let result = chat_stream(&req_id, &provider, &model, messages, opts, on_event, &state_inner, &flag).await;
    let mut map = state.cancel_flags.lock().map_err(|e| e.to_string())?;
    map.remove(&request_id);
    result
}

#[tauri::command]
pub async fn chat_cancel(request_id: String, state: State<'_, AppState>) -> Result<bool, String> {
    let flags = state.cancel_flags.lock().map_err(|e| e.to_string())?;
    if let Some(flag) = flags.get(&request_id) {
        flag.store(true, Ordering::SeqCst);
        Ok(true)
    } else {
        Ok(false)
    }
}

async fn chat_stream(
    request_id: &str,
    provider: &str,
    model: &str,
    messages: Vec<ChatMessage>,
    opts: GenOpts,
    on_event: Channel<ChatEvent>,
    state: &AppState,
    cancelled: &Arc<AtomicBool>,
) -> Result<(), String> {
    let model = model.trim();
    if model.is_empty() {
        let _ = on_event.send(ChatEvent::Error {
            message: "No model selected. Open Settings, choose a model, then retry.".into(),
        });
        return Ok(());
    }
    let base = provider_base(provider, &opts)?;
    // Route the call through CAMEO's own proxy profile unless the caller pinned one.
    // Auto-picked profiles are best-effort: a broken/unreachable proxy must never
    // take chat down, so a network-level failure falls back to a direct connection.
    let proxy_explicit = opts.proxy_profile_id.is_some();
    let proxy_eff: Option<String> = opts
        .proxy_profile_id
        .clone()
        .or_else(|| active_proxy_id(state));
    // Opt-in agent mode runs the tool loop instead of the single streamed call.
    // Ollama stays on the classic path: its local /api/chat shape has no
    // OpenAI-style tool calling for this loop to drive.
    if opts.agent_tools.unwrap_or(false) {
        if provider == "ollama" {
            let _ = on_event.send(ChatEvent::Error {
                message: "Agent tool mode needs an OpenAI-compatible provider (OpenRouter, NVIDIA or Compatible) — Ollama stays on classic chat.".into(),
            });
            return Ok(());
        }
        return agent_tool_loop(request_id, provider, model, messages, opts, on_event, state, cancelled).await;
    }
    let _ = on_event.send(ChatEvent::Status {
        message: format!("Contacting {provider}…"),
    });
    let mut auto_proxy_used = proxy_eff.is_some() && !proxy_explicit;
    let mut client = cached_client(
        proxy_eff.as_deref(),
        state,
        request_timeout(&opts),
    )?;
    let _ = request_id;

    let is_ollama = provider == "ollama";
    let url = if is_ollama {
        format!("{base}/api/chat")
    } else {
        format!("{base}/chat/completions")
    };

    let key = if is_ollama {
        None
    } else {
        match get_key_sync(provider)? {
            Some(k) => Some(k),
            None => {
                let _ = on_event.send(ChatEvent::Error {
                    message: format!("No API key saved for {provider}. Add one in Settings."),
                });
                return Ok(());
            }
        }
    };

    // Skip a model this key was already refused by the provider's tier gate.
    let mut current_model: String = preferred_start_model(
        model,
        prefs_get(state, &verified_model_pref_key(provider)).as_deref(),
        &gated_models(state, provider),
    );
    if current_model != model.trim() {
        let _ = on_event.send(ChatEvent::Delta {
            text: format!(
                "Using '{current_model}' — '{}' is not enabled for this key.\n\n",
                model.trim()
            ),
        });
    }
    let mut attempt = 0usize;
    loop {
        attempt += 1;
        if attempt > 5 {
            let _ = on_event.send(ChatEvent::Error {
                message: format!("Tried 5 fallback models for '{provider}' and none of them answered chat completions with the saved key."),
            });
            return Ok(());
        }
        let body = build_chat_payload(provider, &current_model, &messages, &opts)?;

        let req = {
            let mut r = client.post(&url).json(&body);
            if let Some(k) = &key {
                r = r.bearer_auth(k);
            }
            if provider == "openrouter" {
                r = r.header("HTTP-Referer", "https://cameo.local");
            }
            r
        };

        let resp = match req.send().await {
            Ok(r) => r,
            Err(_) if auto_proxy_used => {
                // Broken auto-picked proxy: drop to a direct connection for
                // this chat (and the fallback probes that may follow).
                let _ = on_event.send(ChatEvent::Status {
                    message: "Proxy unreachable — retrying direct…".into(),
                });
                client = cached_client(None, state, request_timeout(&opts))?;
                auto_proxy_used = false;
                let mut retry = client.post(&url).json(&body);
                if let Some(k) = &key {
                    retry = retry.bearer_auth(k);
                }
                if provider == "openrouter" {
                    retry = retry.header("HTTP-Referer", "https://cameo.local");
                }
                retry.send().await.map_err(|e| format!("network error calling {provider}: {e}"))?
            }
            Err(e) => {
                let _ = e;
                return Err(format!("network error calling {provider}: {e}"));
            }
        };

        let status = resp.status().as_u16();
        if status != 200 {
            let text = resp.text().await.unwrap_or_default();
            // NVIDIA tier gate: catalog lists the model but the account cannot run it.
            let tier_gated =
                status == 404 && provider == "nvidia" && text.contains("Not found for account");
            if tier_gated {
                let _ = on_event.send(ChatEvent::Status {
                    message: "Model not enabled for this key — probing a fallback…".into(),
                });
                if let Some(fallback) = find_served_model(
                    &client,
                    provider,
                    &base,
                    key.as_deref(),
                    &current_model,
                    state,
                )
                .await
                {
                    remember_gated_model(state, provider, &current_model);
                    persist_model_selection(state, provider, &fallback);
                    let _ = on_event.send(ChatEvent::Delta {
                        text: format!(
                            "⚠ '{current_model}' is not enabled for this NVIDIA account tier (HTTP 404). Retrying with '{fallback}', which your key can serve.\n\n"
                        ),
                    });
                    persist_verified_model(state, provider, &fallback);
                    current_model = fallback;
                    continue;
                }
            }
            let msg = http_error(status, &text);
            let _ = on_event.send(ChatEvent::Error { message: msg });
            return Ok(());
        }
        persist_verified_model(state, provider, &current_model);

    use futures_util::StreamExt;
    let mut stream = resp.bytes_stream();
    // `consumed` marks the start of unprocessed bytes. Lines are parsed as
    // borrowed slices and only emitted text is allocated; the consumed
    // prefix is reclaimed in bulk. The old per-line `drain(..=pos)`
    // memmoved the whole remainder on every SSE line, going quadratic on
    // long responses and stalling token delivery.
    let mut buf = String::new();
    let mut consumed: usize = 0;
    let mut prompt_tokens: u64 = 0;
    let mut completion_tokens: u64 = 0;

    while let Some(chunk) = stream.next().await {
        if cancelled.load(Ordering::SeqCst) {
            let _ = on_event.send(ChatEvent::Error { message: "Cancelled".into() });
            return Ok(());
        }
        let bytes = chunk.map_err(|e| format!("stream interrupted: {e}"))?;
        buf.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(rel) = buf[consumed..].find('\n') {
            let end = consumed + rel;
            let line = buf[consumed..end].trim_end_matches('\r');
            consumed = end + 1;
            if line.is_empty() {
                continue;
            }
            if is_ollama {
                if let Ok(v) = serde_json::from_str::<Value>(line) {
                    let text = v["message"]["content"].as_str().unwrap_or("").to_string();
                    if !text.is_empty() {
                        let _ = on_event.send(ChatEvent::Delta { text });
                        completion_tokens += 1; // approximate token accounting
                    }
                    if v["done"].as_bool() == Some(true) {
                        if let Some(ev) = v["prompt_eval_count"].as_u64() {
                            prompt_tokens = ev;
                        }
                        if let Some(ev) = v["eval_count"].as_u64() {
                            completion_tokens = ev;
                        }
                    }
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim();
                if data == "[DONE]" {
                    if prompt_tokens > 0 || completion_tokens > 0 {
                        let _ = on_event.send(ChatEvent::Usage { prompt_tokens, completion_tokens });
                    }
                    let _ = on_event.send(ChatEvent::Done);
                    return Ok(());
                }
                if let Ok(v) = serde_json::from_str::<Value>(data) {
                    let delta = &v["choices"][0]["delta"];
                    // Reasoning traces arrive here on reasoning models
                    // (DeepSeek `reasoning_content`, OpenRouter `reasoning`);
                    // without this the UI sits silent through the whole trace.
                    let mut thinking = delta["reasoning_content"].as_str().unwrap_or("").to_string();
                    if thinking.is_empty() {
                        thinking = match &delta["reasoning"] {
                            Value::String(s) => s.clone(),
                            Value::Array(parts) => parts.iter().filter_map(|p| {
                                p["text"].as_str().map(|s| s.to_string()).or_else(|| p.as_str().map(|s| s.to_string()))
                            }).collect::<Vec<_>>().join(""),
                            _ => String::new(),
                        };
                    }
                    if !thinking.is_empty() {
                        let _ = on_event.send(ChatEvent::Thinking { text: thinking });
                    }
                    let text = delta["content"].as_str().unwrap_or("").to_string();
                    if !text.is_empty() {
                        let _ = on_event.send(ChatEvent::Delta { text });
                    }
                    if let Some(u) = v["usage"].as_object() {
                        prompt_tokens = u.get("prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(prompt_tokens);
                        completion_tokens = u.get("completion_tokens").and_then(|x| x.as_u64()).unwrap_or(completion_tokens);
                    }
                    if let Some(err) = v["error"]["message"].as_str() {
                        let _ = on_event.send(ChatEvent::Error { message: err.to_string() });
                        return Ok(());
                    }
                }
            }
        }
        // Reclaim the processed prefix in bulk so the buffer stays small
        // without a memmove per line.
        if consumed > 65_536 {
            buf.drain(..consumed);
            consumed = 0;
        }
    }
    if prompt_tokens > 0 || completion_tokens > 0 {
        let _ = on_event.send(ChatEvent::Usage { prompt_tokens, completion_tokens });
    }
    let _ = on_event.send(ChatEvent::Done);
        return Ok(());
    }
}
