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
    Usage { prompt_tokens: u64, completion_tokens: u64 },
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
        .connect_timeout(Duration::from_secs(15))
        .timeout(request_timeout)
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
        let mut guard = CLIENT_CACHE.lock().map_err(|_| "client cache poisoned")?;
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

/// Pure helper (unit-tested): build the ordered fallback candidate list.
/// Order: last verified model, well-known lightweight defaults, then catalog ids.
/// Always excludes the model that just failed and dedupes; capped at `cap`.
fn fallback_candidates(verified: Option<&str>, catalog: &[String], failed: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |c: Option<String>, out: &mut Vec<String>| {
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
    // Well-known lightweight models that broadly serve across NVIDIA tiers;
    // probed before paying for a catalog fetch.
    const KNOWN_LIGHTWEIGHT: [&str; 3] = [
        "openai/gpt-oss-20b",
        "deepseek-ai/deepseek-v4.1-flash",
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
    for cand in fallback_candidates(None, &catalog, failed) {
        if probe_model_serves(client, base, key, &cand).await {
            persist_verified_model(state, provider, &cand);
            return Some(cand);
        }
    }
    None
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

    let mut current_model: String = model.to_string();
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

        let mut req = {
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
            Err(e) if auto_proxy_used => {
                // Broken auto-picked proxy: drop to a direct connection for
                // this chat (and the fallback probes that may follow).
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
    let mut buf = String::new();
    let mut prompt_tokens: u64 = 0;
    let mut completion_tokens: u64 = 0;

    while let Some(chunk) = stream.next().await {
        if cancelled.load(Ordering::SeqCst) {
            let _ = on_event.send(ChatEvent::Error { message: "Cancelled".into() });
            return Ok(());
        }
        let bytes = chunk.map_err(|e| format!("stream interrupted: {e}"))?;
        buf.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim_end_matches('\r').to_string();
            buf.drain(..=pos);
            if line.is_empty() {
                continue;
            }
            if is_ollama {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
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
                    let text = v["choices"][0]["delta"]["content"].as_str().unwrap_or("").to_string();
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
    }
    if prompt_tokens > 0 || completion_tokens > 0 {
        let _ = on_event.send(ChatEvent::Usage { prompt_tokens, completion_tokens });
    }
    let _ = on_event.send(ChatEvent::Done);
        return Ok(());
    }
}
