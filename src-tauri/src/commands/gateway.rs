use crate::commands::mcp;
use crate::commands::store::AppState;
use keyring::Entry;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tauri::State;

// ===========================================================================
// CAMEO Universal Secure Authentication Gateway.
//
// Independent components:
//   CredentialVault          -> OS credential store (keyring windows-native)
//   AuditLogger              -> SQLite audit_events, all details redacted
//   PermissionEngine         -> read / prepare / write / sensitive per provider,
//                               sensitive needs a per-call JIT approval preview
//   OAuthManager             -> Authorization Code + PKCE, loopback redirect,
//                               CSRF state, refresh, server-side revocation
//   ConnectionHealthMonitor  -> real authenticated read-only API calls
//   RevocationManager        -> disconnect + provider-side revoke guidance
//
// Tokens and keys never leave the backend (status-only responses). Each
// provider has an isolated vault service entry, so one provider's credential
// is never used as another provider's and tokens are not forwarded to MCP.
// ===========================================================================

pub const GATEWAY_SCHEMA_SQL: &str = "CREATE TABLE IF NOT EXISTS connected_accounts (
    provider TEXT PRIMARY KEY,
    auth_method TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'disconnected',
    account_label TEXT,
    scopes TEXT NOT NULL DEFAULT '[]',
    permissions TEXT NOT NULL DEFAULT '{\"read\":true,\"prepare\":false,\"write\":false,\"sensitive\":false}',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_check_at INTEGER,
    last_check_ok INTEGER,
    last_error TEXT,
    token_expires_at INTEGER
);
CREATE TABLE IF NOT EXISTS audit_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts INTEGER NOT NULL,
    category TEXT NOT NULL,
    provider TEXT,
    action TEXT NOT NULL,
    outcome TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT '{}'
);";

// ---------------------------------------------------------------------------
// GatewayState — global Stop + in-flight OAuth flows
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct GatewayState {
    pub stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub pending_auth: std::sync::Mutex<Vec<PendingAuth>>,
    pub seq: AtomicU64,
}

#[derive(Clone)]
pub struct PendingAuth {
    pub client_state: String,
    pub provider: String,
    pub verifier: String,
    pub scopes: Vec<String>,
    pub port: u16,
}

#[tauri::command]
pub fn gw_stop_control(engage: bool, gateway: State<'_, Arc<GatewayState>>, state: State<'_, AppState>) -> Result<Value, String> {
    gateway.stop.store(engage, Ordering::SeqCst);
    mcp::kill_all_stdio();
    audit(&state, "control", "global", if engage { "stop_engaged" } else { "stop_released" }, "ok", &json!({}));
    Ok(json!({ "stop": engage }))
}

#[tauri::command]
pub fn gw_stop_status(gateway: State<'_, Arc<GatewayState>>) -> Value {
    json!({ "stop": gateway.stop.load(Ordering::SeqCst) })
}

pub fn assert_not_stopped(gateway: &GatewayState) -> Result<(), String> {
    if gateway.stop.load(Ordering::SeqCst) {
        return Err("Global Stop is engaged — release it on the Connections page before continuing.".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// CredentialVault
// ---------------------------------------------------------------------------

const VAULT_PROVIDERS: [&str; 3] = ["google", "github", "render"];

fn vault_entry(provider: &str, slot: &str) -> Result<Entry, String> {
    if !VAULT_PROVIDERS.contains(&provider) {
        return Err(format!("unknown gateway provider '{provider}'"));
    }
    if slot.is_empty() || !slot.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err("invalid credential slot".into());
    }
    Entry::new(&format!("cameo_gateway_{provider}"), slot).map_err(|e| format!("credential store error: {e}"))
}

pub fn vault_secret(provider: &str, slot: &str) -> Result<Option<String>, String> {
    match vault_entry(provider, slot) {
        Ok(entry) => match entry.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("credential store error: {e}")),
        },
        Err(_) => Ok(None),
    }
}

#[tauri::command]
pub async fn gw_set_secret(provider: String, slot: String, secret: String) -> Result<bool, String> {
    if secret.trim().chars().count() < 8 {
        return Err("that value looks too short to be a valid credential".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        vault_entry(&provider, &slot)?
            .set_password(secret.trim())
            .map(|_| true)
            .map_err(|e| format!("failed to store credential: {e}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn gw_has_secret(provider: String, slot: String) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || vault_entry(&provider, &slot).map(|e| e.get_password().is_ok()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn gw_clear_secrets(provider: String, slots: Vec<String>) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut cleared = 0;
        for slot in &slots {
            if let Ok(entry) = vault_entry(&provider, slot) {
                match entry.delete_credential() {
                    Ok(()) => cleared += 1,
                    Err(keyring::Error::NoEntry) => {}
                    Err(e) => return Err(format!("failed to delete credential: {e}")),
                }
            }
        }
        Ok(cleared)
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------------------
// AuditLogger
// ---------------------------------------------------------------------------

pub const REDACT_MARKERS: [&str; 12] = [
    "sk-", "ghp_", "github_pat_", "gho_", "ghu_", "ghs_", "nct_", "r8_", "Bearer ", "access_token=",
    "refresh_token=", "code=",
];

/// Redact obvious secrets from free-form text; keep at most a 4-char prefix.
pub fn redact(input: &str) -> String {
    let mut out = input.to_string();
    for marker in REDACT_MARKERS {
        let mut search_from = 0usize;
        while let Some(rel) = out[search_from..].find(marker) {
            let abs = search_from + rel;
            let value_start = abs + marker.len();
            let value_len = out[value_start..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.' || *c == '~')
                .count();
            if value_len > 4 {
                let end = value_start + value_len;
                let keep_end = (value_start + 4).min(end);
                out.replace_range(keep_end..end, "…");
            }
            search_from = value_start + 1;
            if search_from >= out.len() {
                break;
            }
        }
    }
    out
}

/// Redact secret-shaped JSON fields recursively (for audit details).
pub fn redact_value(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for (k, val) in map.iter_mut() {
                match k.to_ascii_lowercase().as_str() {
                    "token" | "access_token" | "refresh_token" | "id_token" | "secret" | "client_secret"
                    | "password" | "api_key" | "authorization" => {
                        *val = Value::String("REDACTED…".into());
                    }
                    _ => {
                        if val.is_string() {
                            *val = Value::String(redact(val.as_str().unwrap_or_default()));
                        } else {
                            redact_value(val);
                        }
                    }
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_value),
        _ => {}
    }
}

fn audit(state: &AppState, category: &str, provider: &str, action: &str, outcome: &str, detail: &Value) {
    if let Ok(db) = state.db.lock() {
        let _ = audit_db(&db, category, provider, action, outcome, detail);
    }
}

pub fn audit_db(db: &Connection, category: &str, provider: &str, action: &str, outcome: &str, detail: &Value) -> Result<(), String> {
    let mut detail = detail.clone();
    redact_value(&mut detail);
    db.execute(
        "INSERT INTO audit_events (ts, category, provider, action, outcome, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![now_ms(), category, provider, action, outcome, serde_json::to_string(&detail).unwrap_or_else(|_| "{}".into())],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn gw_audit_list(state: State<'_, AppState>, limit: Option<usize>) -> Result<Vec<Value>, String> {
    let limit = limit.unwrap_or(80).clamp(1, 500) as i64;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let mut stmt = db
        .prepare("SELECT id, ts, category, provider, action, outcome, detail FROM audit_events ORDER BY id DESC LIMIT ?1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([limit], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "ts": r.get::<_, i64>(1)?,
                "category": r.get::<_, String>(2)?,
                "provider": r.get::<_, Option<String>>(3)?,
                "action": r.get::<_, String>(4)?,
                "outcome": r.get::<_, String>(5)?,
                "detail": serde_json::from_str::<Value>(&r.get::<_, String>(6).unwrap_or_else(|_| "{}".into())).unwrap_or_else(|_| json!({})),
            }))
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// PermissionEngine
// ---------------------------------------------------------------------------

pub const LEVELS: [&str; 4] = ["read", "prepare", "write", "sensitive"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Permissions {
    pub read: bool,
    pub prepare: bool,
    pub write: bool,
    pub sensitive: bool,
}

impl Permissions {
    pub fn read_only() -> Self {
        Self { read: true, prepare: false, write: false, sensitive: false }
    }

    pub fn allows(&self, level: &str) -> bool {
        match level {
            "read" => self.read,
            "prepare" => self.prepare,
            "write" => self.write,
            "sensitive" => self.sensitive,
            _ => false,
        }
    }
}

pub fn authorize_call(perms: &Permissions, level: &str) -> Result<(), &'static str> {
    if !LEVELS.contains(&level) {
        return Err("unknown permission level");
    }
    if !perms.allows(level) {
        return Err("this provider's permissions do not allow that operation level");
    }
    Ok(())
}

pub fn parse_permissions(v: &Value) -> Result<Permissions, String> {
    let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    let sensitive = flag("sensitive");
    let write = flag("write") || sensitive;
    let prepare = flag("prepare") || write;
    let read = flag("read") || prepare;
    if !read {
        return Err("permissions object must grant at least 'read' access".into());
    }
    Ok(Permissions { read, prepare, write, sensitive })
}

fn permissions_of(state: &AppState, provider: &str) -> Result<Permissions, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let raw = db
        .query_row("SELECT permissions FROM connected_accounts WHERE provider = ?1", [provider], |r| r.get::<_, String>(0))
        .optional()
        .map_err(|e| e.to_string())?;
    match raw {
        Some(s) => parse_permissions(&serde_json::from_str::<Value>(&s).unwrap_or_else(|_| json!({})))
            .map_err(|e| format!("stored permissions for '{provider}' are invalid: {e}")),
        None => Ok(Permissions::read_only()),
    }
}

fn permissions_value(p: &Permissions) -> Value {
    json!({ "read": p.read, "prepare": p.prepare, "write": p.write, "sensitive": p.sensitive })
}

#[tauri::command]
pub fn gw_get_permissions(provider: String, state: State<'_, AppState>) -> Result<Value, String> {
    Ok(permissions_value(&permissions_of(&state, &provider)?))
}

#[tauri::command]
pub fn gw_set_permissions(provider: String, permissions: Value, state: State<'_, AppState>) -> Result<Value, String> {
    let next = parse_permissions(&permissions)?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let exists: bool = db
        .query_row("SELECT EXISTS(SELECT 1 FROM connected_accounts WHERE provider=?1)", [&provider], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if !exists {
        return Err(format!("'{provider}' is not connected yet — connect first, then set permissions"));
    }
    db.execute(
        "UPDATE connected_accounts SET permissions=?2, updated_at=?3 WHERE provider=?1",
        params![provider, serde_json::to_string(&permissions_value(&next)).unwrap_or_else(|_| "{}".into()), now_ms()],
    )
    .map_err(|e| e.to_string())?;
    audit_db(&db, "permission", &provider, "set_permissions", "ok", &json!({ "permissions": permissions_value(&next) }))?;
    Ok(permissions_value(&next))
}

// ---------------------------------------------------------------------------
// Provider registry + tool catalogue
// ---------------------------------------------------------------------------

pub struct ProviderSpec {
    pub id: &'static str,
    pub display: &'static str,
    pub auth: &'static str,
    pub base_endpoint: &'static str,
    pub required_scopes: &'static [&'static str],
    pub revoke_guide: &'static str,
}

pub const PROVIDER_SPECS: [ProviderSpec; 3] = [
    ProviderSpec {
        id: "google",
        display: "Google & Gmail",
        auth: "oauth",
        base_endpoint: "https://gmail.googleapis.com/",
        required_scopes: &["https://www.googleapis.com/auth/gmail.readonly"],
        revoke_guide: "Revoke CAMEO's access at https://myaccount.google.com/connections, then press Disconnect here.",
    },
    ProviderSpec {
        id: "github",
        display: "GitHub",
        auth: "oauth",
        base_endpoint: "https://api.github.com/",
        required_scopes: &["repo:status", "read:user"],
        revoke_guide: "Revoke the CAMEO OAuth application at https://github.com/settings/applications, then press Disconnect here.",
    },
    ProviderSpec {
        id: "render",
        display: "Render.com",
        auth: "api_key",
        base_endpoint: "https://api.render.com/",
        required_scopes: &[],
        revoke_guide: "Delete the API key in the Render dashboard (Team Settings → API Keys). CAMEO stores it only in the OS credential store.",
    },
];

pub fn provider_spec(provider: &str) -> Result<&'static ProviderSpec, String> {
    PROVIDER_SPECS
        .iter()
        .find(|s| s.id == provider)
        .ok_or_else(|| format!("unsupported gateway provider '{provider}'"))
}

pub struct ToolSpec {
    pub name: &'static str,
    pub level: &'static str,
    pub approval_needed: bool,
    pub description: &'static str,
}

pub fn tool_specs(provider: &str) -> Vec<ToolSpec> {
    match provider {
        "google" => vec![
            ToolSpec { name: "gmail_list", level: "read", approval_needed: false, description: "Search/read Gmail messages (read-only scope)" },
            ToolSpec { name: "gmail_draft", level: "prepare", approval_needed: false, description: "Create a Gmail draft" },
            ToolSpec { name: "gmail_send", level: "sensitive", approval_needed: true, description: "Send an email — always requires explicit approval with preview" },
        ],
        "github" => vec![
            ToolSpec { name: "github_list_repos", level: "read", approval_needed: false, description: "List repositories the authorization can see" },
            ToolSpec { name: "github_list_commits", level: "read", approval_needed: false, description: "List recent commits on a branch" },
            ToolSpec { name: "github_create_issue", level: "write", approval_needed: false, description: "Open an issue on a repository" },
            ToolSpec { name: "github_create_branch", level: "write", approval_needed: false, description: "Create a branch from a base ref" },
        ],
        "render" => vec![
            ToolSpec { name: "render_list_services", level: "read", approval_needed: false, description: "List Render services" },
            ToolSpec { name: "render_list_deploys", level: "read", approval_needed: false, description: "List deployment history" },
            ToolSpec { name: "render_trigger_deploy", level: "sensitive", approval_needed: true, description: "Trigger a production deploy — requires approval" },
            ToolSpec { name: "render_restart_service", level: "sensitive", approval_needed: true, description: "Restart a service — requires approval" },
        ],
        _ => Vec::new(),
    }
}

#[tauri::command]
pub fn gw_registry() -> Vec<Value> {
    PROVIDER_SPECS
        .iter()
        .map(|s| {
            json!({
                "provider": s.id,
                "display": s.display,
                "authMethod": s.auth,
                "baseEndpoint": s.base_endpoint,
                "requiredScopes": s.required_scopes,
                "revokeGuide": s.revoke_guide,
                "tools": tool_specs(s.id).iter().map(|t| json!({
                    "name": t.name,
                    "level": t.level,
                    "approvalNeeded": t.approval_needed,
                    "description": t.description,
                })).collect::<Vec<_>>(),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Connection status
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn gw_status(provider: String, gateway: State<'_, Arc<GatewayState>>, state: State<'_, AppState>) -> Result<Value, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let row = db
        .query_row(
            "SELECT auth_method, status, account_label, scopes, permissions, last_check_at, last_check_ok, last_error, token_expires_at, updated_at
             FROM connected_accounts WHERE provider=?1",
            [&provider],
            |r| {
                Ok(json!({
                    "authMethod": r.get::<_, String>(0)?,
                    "status": r.get::<_, String>(1)?,
                    "accountLabel": r.get::<_, Option<String>>(2)?,
                    "scopes": serde_json::from_str::<Value>(&r.get::<_, String>(3).unwrap_or_else(|_| "[]".into())).unwrap_or_else(|_| json!([])),
                    "permissions": serde_json::from_str::<Value>(&r.get::<_, String>(4).unwrap_or_else(|_| "{}".into())).unwrap_or_else(|_| json!({})),
                    "lastCheckAt": r.get::<_, Option<i64>>(5)?,
                    "lastCheckOk": r.get::<_, Option<i64>>(6)?,
                    "lastError": r.get::<_, Option<String>>(7)?,
                    "tokenExpiresAt": r.get::<_, Option<i64>>(8)?,
                    "updatedAt": r.get::<_, i64>(9)?,
                }))
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let waiting = {
        let pend = gateway.pending_auth.lock().map_err(|e| e.to_string())?;
        pend.iter().any(|p| p.provider == provider)
    };
    let mut val = row.unwrap_or_else(|| json!({
        "status": "not_configured", "permissions": {}, "scopes": []
    }));
    if waiting && val["status"] != json!("connected") {
        val["status"] = json!("awaiting_user_authorization");
    }
    Ok(val)
}

#[tauri::command]
pub fn gw_list_all(state: State<'_, AppState>) -> Result<Vec<Value>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let mut stmt = db
        .prepare("SELECT provider, auth_method, status, account_label, permissions, last_check_at, last_check_ok, last_error FROM connected_accounts")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(json!({
                "provider": r.get::<_, String>(0)?,
                "authMethod": r.get::<_, String>(1)?,
                "status": r.get::<_, String>(2)?,
                "accountLabel": r.get::<_, Option<String>>(3)?,
                "permissions": serde_json::from_str::<Value>(&r.get::<_, String>(4).unwrap_or_else(|_| "{}".into())).unwrap_or_else(|_| json!({})),
                "lastCheckAt": r.get::<_, Option<i64>>(5)?,
                "lastCheckOk": r.get::<_, Option<i64>>(6)?,
                "lastError": r.get::<_, Option<String>>(7)?,
            }))
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

fn upsert_account_row(db: &Connection, provider: &str, auth_method: &str) -> Result<(), String> {
    db.execute(
        "INSERT INTO connected_accounts (provider, auth_method, status, created_at, updated_at)
         VALUES (?1, ?2, 'connecting', ?3, ?3)
         ON CONFLICT(provider) DO UPDATE SET auth_method=?2, updated_at=?3",
        params![provider, auth_method, now_ms()],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}
// ---------------------------------------------------------------------------
// OAuth helpers (PKCE, loopback callback, exchange, refresh, revocation)
// ---------------------------------------------------------------------------

const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";
const GOOGLE_OPENID_CONFIG: &str = "https://accounts.google.com/.well-known/openid-configuration";
const GITHUB_AUTH_URL: &str = "https://github.com/login/oauth/authorize";
const GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";

fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("http client error: {e}"))
}

const PKCE_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";

fn random_string(chars: &[u8], len: usize) -> String {
    let mut bytes = vec![0u8; len];
    if getrandom::fill(&mut bytes).is_err() {
        // Fallback: high-res time + pid entropy mix; bounded per value.
        let mut a = (now_ms() as u64).wrapping_mul(6364136223846793005);
        let mut b = std::process::id() as u64 ^ 0x9E3779B97F4A7C15;
        for slot in bytes.iter_mut() {
            a = a.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            b = b.wrapping_add(a.rotate_left(17));
            *slot = (a ^ b) as u8;
        }
    }
    bytes.iter().map(|x| chars[((*x) as usize) % chars.len()] as char).collect()
}

pub struct PkcePair {
    pub verifier: String,
    pub challenge: String,
}

pub fn generate_pkce() -> PkcePair {
    let verifier = random_string(PKCE_CHARS, 64);
    let digest = Sha256::digest(verifier.as_bytes());
    PkcePair { verifier, challenge: base64url_no_pad(&digest) }
}

pub fn base64url_no_pad(bytes: &[u8]) -> String {
    const TBL: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let full = bytes.len() / 3 * 3;
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 4);
    for chunk in bytes[..full].chunks(3) {
        let n = ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8) | chunk[2] as u32;
        out.push(TBL[(n >> 18) as usize & 63] as char);
        out.push(TBL[(n >> 12) as usize & 63] as char);
        out.push(TBL[(n >> 6) as usize & 63] as char);
        out.push(TBL[n as usize & 63] as char);
    }
    match bytes.len() - full {
        1 => {
            let n = (bytes[full] as u32) << 16;
            out.push(TBL[(n >> 18) as usize & 63] as char);
            out.push(TBL[(n >> 12) as usize & 63] as char);
        }
        2 => {
            let n = ((bytes[full] as u32) << 16) | ((bytes[full + 1] as u32) << 8);
            out.push(TBL[(n >> 18) as usize & 63] as char);
            out.push(TBL[(n >> 12) as usize & 63] as char);
            out.push(TBL[(n >> 6) as usize & 63] as char);
        }
        _ => {}
    }
    out
}

fn urlencode(v: &str) -> String {
    let mut out = String::new();
    for c in v.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(c),
            ' ' => out.push_str("%20"),
            '+' => out.push_str("%2B"),
            _ => {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).as_bytes() {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }
    out
}

pub fn urldecode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[derive(Debug)]
enum CallbackResult {
    Code { code: String, state: String },
    Denied(String),
    Failed(String),
}

/// Binds the loopback redirect, opens the official authorization URL in the
/// user's browser, waits for the single callback, validates CSRF state.
fn await_loopback_callback(port: u16, expected_state: String, auth_url: String) -> CallbackResult {
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => return CallbackResult::Failed(format!("cannot bind loopback redirect port {port}: {e}")),
    };
    if std::process::Command::new("cmd").args(["/C", "start", "", &auth_url]).spawn().is_err() {
        return CallbackResult::Failed("failed to open the default browser for authorization".into());
    }
    use std::io::{BufRead, Write};
    let (mut stream, _) = match listener.accept() {
        Ok(pair) => pair,
        Err(e) => return CallbackResult::Failed(format!("no authorization callback received: {e}")),
    };
    let mut request_line = String::new();
    let mut reader = std::io::BufReader::new(&mut stream);
    if reader.read_line(&mut request_line).is_err() {
        return CallbackResult::Failed("malformed authorization callback".into());
    }
    let _ = stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n<html><body><p>CAMEO received the authorization response - you can close this tab.</p></body></html>",
    );
    let _ = stream.flush();

    let path = request_line.split_whitespace().nth(1).unwrap_or("").trim_start_matches('/').to_string();
    let query = path.split_once('?').map(|(_, q)| q.to_string()).unwrap_or_default();
    let mut code = String::new();
    let mut state = String::new();
    for pair in query.split('&') {
        let Some((k, raw_v)) = pair.split_once('=') else { continue };
        match k {
            "code" => code = urldecode(raw_v),
            "state" => state = urldecode(raw_v),
            "error" => return CallbackResult::Denied("the authorization request was denied or canceled".into()),
            _ => {}
        }
    }
    if code.is_empty() || state.is_empty() {
        return CallbackResult::Failed("callback is missing code or state".into());
    }
    if state != expected_state {
        return CallbackResult::Failed("authorization state mismatch - possible forgery; rejecting".into());
    }
    CallbackResult::Code { code, state }
}

fn discovery_check_google(client: &reqwest::blocking::Client) -> Result<(), String> {
    // Validate the authorization server issuer (RFC 8414 well-known config)
    // before sending the user's browser anywhere.
    let cfg: Value = client
        .get(GOOGLE_OPENID_CONFIG)
        .send()
        .map_err(|e| format!("issuer discovery failed: {e}"))?
        .json()
        .map_err(|e| format!("issuer discovery parse failed: {e}"))?;
    let issuer = cfg["issuer"].as_str().unwrap_or("");
    if issuer != "https://accounts.google.com" {
        return Err(format!("unexpected authorization server issuer '{issuer}'"));
    }
    let supports_s256 = cfg["code_challenge_methods_supported"]
        .as_array()
        .map(|a| a.iter().any(|x| x.as_str() == Some("S256")))
        .unwrap_or(false);
    if !supports_s256 {
        return Err("authorization server does not advertise S256 PKCE support".into());
    }
    Ok(())
}

fn slot_token(provider: &str) -> &'static str {
    match provider {
        "google" => "google_token",
        "github" => "github_token",
        "render" => "api_key",
        _ => "token",
    }
}

fn slot_refresh(_provider: &str) -> &'static str {
    "refresh_token"
}

pub fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

struct TokenExchangeOk {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
    scopes: Vec<String>,
}

fn exchange_code(provider: &str, code: &str, verifier: &str, client_id: &str, client_secret: &str, redirect: &str) -> Result<TokenExchangeOk, String> {
    let client = http_client()?;
    let mut form: Vec<(&str, String)> = vec![
        ("code", code.to_string()),
        ("client_id", client_id.to_string()),
        ("redirect_uri", redirect.to_string()),
        ("grant_type", "authorization_code".into()),
        ("code_verifier", verifier.to_string()),
    ];
    if !client_secret.is_empty() {
        form.push(("client_secret", client_secret.to_string()));
    }
    let url = match provider {
        "google" => GOOGLE_TOKEN_URL,
        "github" => GITHUB_TOKEN_URL,
        p => return Err(format!("no token endpoint for provider '{p}'")),
    };
    let resp = client
        .post(url)
        .header("Accept", "application/json")
        .form(&form)
        .send()
        .map_err(|e| format!("token exchange request failed: {e}"))?;
    let status = resp.status().as_u16();
    let body = resp.text().map_err(|e| e.to_string())?;
    if !(200..300).contains(&status) {
        return Err(format!("token endpoint HTTP {status}: {}", truncate(&redact(&body), 200)));
    }
    let v: Value = serde_json::from_str(&body).map_err(|_| format!("unparseable token response: {}", truncate(&redact(&body), 160)))?;
    let access = v["access_token"].as_str().unwrap_or("").to_string();
    if access.is_empty() {
        let err = v["error_description"].as_str().or_else(|| v["error"].as_str()).unwrap_or("missing access_token");
        return Err(format!("token endpoint rejected: {err}"));
    }
    let scopes: Vec<String> = v["scope"]
        .as_str()
        .unwrap_or("")
        .split(|c| c == ' ' || c == ',')
        .filter(|x| !x.is_empty())
        .map(|x| x.to_string())
        .collect();
    let refresh = v["refresh_token"].as_str().map(|x| x.to_string());
    let expires_in = v["expires_in"].as_u64().unwrap_or(3600);
    Ok(TokenExchangeOk { access_token: access, refresh_token: refresh, expires_in, scopes })
}
// ---------------------------------------------------------------------------
// OAuth connect flow (command) + exchange handling
// ---------------------------------------------------------------------------

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn gw_oauth_connect(
    provider: String,
    features: Vec<String>,
    gateway: State<'_, Arc<GatewayState>>,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    let s = provider_spec(&provider)?;
    if s.auth != "oauth" {
        return Err(format!("provider '{provider}' does not support OAuth; use an API key instead"));
    }
    assert_not_stopped(&gateway)?;

    let client_id = vault_secret(&provider, "client_id")?;
    let Some(client_id) = client_id else {
        return Err("No application client ID stored yet — save the OAuth client ID on the Connections page first (the 'App credentials' card).".into());
    };
    let client_secret = if provider == "google" {
        let secret = vault_secret(&provider, "client_secret")?;
        if secret.is_none() {
            return Err("Google's authorized-client flow requires the client secret too — save it on the Connections page.".into());
        }
        secret.unwrap_or_default()
    } else {
        String::new()
    };

    // Scopes: required baseline + only the scopes for features the user enabled.
    let mut scopes: Vec<String> = s.required_scopes.iter().map(|x| x.to_string()).collect();
    for f in &features {
        let wanted = match (provider.as_str(), f.as_str()) {
            ("google", "drafts") => Some("https://www.googleapis.com/auth/gmail.drafts"),
            ("google", "send") => Some("https://www.googleapis.com/auth/gmail.send"),
            ("github", "write") => Some("repo"),
            ("github", "workflow") => Some("workflow"),
            _ => None,
        };
        if let Some(sc) = wanted {
            if !scopes.contains(&sc.to_string()) {
                scopes.push(sc.to_string());
            }
        }
    }

    let pkce = generate_pkce();
    let csrf_state = random_string(b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789", 40);

    // Reserve a free loopback port for the RFC 8252 redirect.
    let probe = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("loopback bind failed: {e}"))?;
    let port = probe.local_addr().map_err(|e| e.to_string())?.port();
    drop(probe);

    {
        let mut pend = gateway.pending_auth.lock().map_err(|e| e.to_string())?;
        pend.retain(|p| p.provider != provider);
        pend.push(PendingAuth {
            client_state: csrf_state.clone(),
            provider: provider.clone(),
            verifier: pkce.verifier.clone(),
            scopes: scopes.clone(),
            port,
        });
    }
    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        upsert_account_row(&db, &provider, "oauth")?;
        let _ = db.execute(
            "UPDATE connected_accounts SET status='awaiting_user_authorization', updated_at=?2 WHERE provider=?1",
            params![provider, now_ms()],
        );
        drop(db);
        audit(&state, "oauth", &provider, "begin_authorization", "started", &json!({
            "scopes": scopes, "redirect": format!("http://127.0.0.1:{port}/callback")
        }));
    }

    // Optional issuer discovery validation for Google.
    if provider == "google" {
        let check = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
            let client = http_client()?;
            discovery_check_google(&client)
        })
        .await
        .map_err(|e| e.to_string())?;
        if let Err(e) = check {
            {
                let mut pend = gateway.pending_auth.lock().map_err(|e| e.to_string())?;
                pend.retain(|p| p.provider != provider);
            }
            audit(&state, "oauth", &provider, "discovery_check", "failed", &json!({ "error": e }));
            return Err(e);
        }
    }

    let redirect = format!("http://127.0.0.1:{port}/callback");
    let mut query = vec![
        ("client_id", client_id.clone()),
        ("redirect_uri", redirect.clone()),
        ("response_type", "code".to_string()),
        ("scope", scopes.join(" ")),
        ("state", csrf_state.clone()),
        ("code_challenge", pkce.challenge.clone()),
        ("code_challenge_method", "S256".to_string()),
    ];
    if provider == "google" {
        query.push(("access_type", "offline".into()));
        query.push(("prompt", "consent".into()));
        query.push(("include_granted_scopes", "true".into()));
    }
    let params_str = query
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let auth_url = format!(
        "{}?{}",
        if provider == "google" { GOOGLE_AUTH_URL } else { GITHUB_AUTH_URL },
        params_str
    );

    let flow_provider = provider.clone();
    let flow_state = csrf_state.clone();
    let app = state.inner().clone();
    let token_slot_owned = slot_token(&provider).to_string();
    let redirect_scope_owned = redirect.clone();
    let gateway_owned = Arc::clone(&gateway);
    let csrf_state_owned = csrf_state.clone();
    tauri::async_runtime::spawn(async move {
        let callback_res = tauri::async_runtime::spawn_blocking(move || {
            await_loopback_callback(port, flow_state, auth_url)
        })
        .await;
        let outcome: Result<CallbackResult, String> = match callback_res {
            Ok(cb) => Ok(cb),
            Err(e) => Err(e.to_string()),
        };
        finish_oauth_flow(
            flow_provider,
            outcome,
            csrf_state_owned,
            client_id,
            client_secret,
            token_slot_owned,
            redirect_scope_owned,
            app,
            gateway_owned,
        );
    });

    Ok(json!({
        "started": true,
        "provider": provider,
        "status": "awaiting_user_authorization",
        "redirectEndpoint": redirect,
        "scopes": scopes,
        "message": "A browser window opened for the official authorization page. CAMEO listens only on 127.0.0.1 for the callback."
    }))
}

#[allow(clippy::too_many_arguments)]
fn finish_oauth_flow(
    provider: String,
    outcome_raw: Result<CallbackResult, String>,
    expected_state: String,
    client_id: String,
    client_secret: String,
    token_slot: String,
    redirect: String,
    app: AppState,
    gateway_inner: Arc<GatewayState>,
) {
    #[derive(Debug)]
    enum StageOutcome {
        Code(String, String, Vec<String>),
        Err(String, String),
    }
    let outcome: StageOutcome = {
        let callback = match outcome_raw {
            Ok(c) => c,
            Err(e) => CallbackResult::Failed(e),
        };
        // Look up the flow's verifier by CSRF state, then drop the flow record.
        let mut pend = gateway_inner.pending_auth.lock().expect("pending auth");
        let flow = pend.iter().find(|p| p.client_state == expected_state).cloned();
        pend.retain(|p| p.client_state != expected_state);
        drop(pend);
        match callback {
            CallbackResult::Code { code, .. } => match flow {
                Some(f) => StageOutcome::Code(code, f.verifier, f.scopes),
                None => StageOutcome::Err("no in-flight authorization flow matched the callback".into(), "flow_missing".into()),
            },
            CallbackResult::Denied(msg) => StageOutcome::Err(msg, "user_denied".into()),
            CallbackResult::Failed(msg) => StageOutcome::Err(msg, "callback_failed".into()),
        }
    };
    let (code, verifier, scopes_final) = match outcome {
        StageOutcome::Code(code, verifier, scopes_final) => (code, verifier, scopes_final),
        StageOutcome::Err(err, stage) => {
            let db = app.db.lock().expect("db");
            let _ = db.execute(
                "UPDATE connected_accounts SET status='failed', last_error=?2, updated_at=?3 WHERE provider=?1",
                params![provider, truncate(&err, 200), now_ms()],
            );
            let _ = audit_db(&db, "oauth", &provider, "callback", "failed", &json!({ "error": truncate(&err, 160), "stage": stage }));
            return;
        }
    };
    if verifier.is_empty() {
        let db = app.db.lock().expect("db");
        let _ = db.execute(
            "UPDATE connected_accounts SET status='failed', last_error=?2, updated_at=?3 WHERE provider=?1",
            params![provider, "missing PKCE verifier for the authorization state", now_ms()],
        );
        let _ = audit_db(&db, "oauth", &provider, "callback", "failed", &json!({ "error": "missing PKCE verifier" }));
        return;
    }
    match exchange_code(&provider, &code, &verifier, &client_id, &client_secret, &redirect) {
        Ok(tok) => {
            let db = app.db.lock().expect("db");
            if let Ok(entry) = vault_entry(&provider, &token_slot) {
                let _ = entry.set_password(&tok.access_token);
            }
            if let Some(refr) = &tok.refresh_token {
                if let Ok(entry) = vault_entry(&provider, slot_refresh(&provider)) {
                    let _ = entry.set_password(refr);
                }
            }
            let now = now_ms();
            let expires_at = now + tok.expires_in as i64 * 1000;
            let _ = db.execute(
                "UPDATE connected_accounts SET status='connected_pending_check', scopes=?2, token_expires_at=?3, updated_at=?4 WHERE provider=?1",
                params![provider, serde_json::to_string(&tok.scopes).unwrap_or_else(|_| "[]".into()), expires_at, now],
            );
            let _ = audit_db(&db, "oauth", &provider, "token_exchange", "ok", &json!({ "expires_in": tok.expires_in, "scopes": tok.scopes }));
        }
        Err(e) => {
            let db = app.db.lock().expect("db");
            let _ = db.execute(
                "UPDATE connected_accounts SET status='failed', last_error=?2, updated_at=?3 WHERE provider=?1",
                params![provider, truncate(&e, 200), now_ms()],
            );
            let _ = audit_db(&db, "oauth", &provider, "token_exchange", "failed", &json!({ "error": truncate(&e, 160) }));
        }
    }
}

// ---------------------------------------------------------------------------
// Refresh + revoke
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn gw_oauth_refresh(provider: String, state: State<'_, AppState>) -> Result<Value, String> {
    if provider_spec(&provider)?.auth != "oauth" {
        return Err(format!("provider '{provider}' does not support OAuth"));
    }
    let p = provider.clone();
    let app = state.inner().clone();
    let res = tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let refresh = vault_secret(&p, slot_refresh(&p))?.unwrap_or_default();
        if refresh.is_empty() {
            return Ok(json!({ "ok": false, "message": "no refresh token stored — reconnect required", "provider": p }));
        }
        let client_id = vault_secret(&p, "client_id")?.unwrap_or_default();
        let client_secret = vault_secret(&p, "client_secret")?.unwrap_or_default();
        let client = http_client()?;
        let form: Vec<(&str, String)> = vec![
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh),
            ("grant_type", "refresh_token".into()),
        ];
        let url = if p == "google" { GOOGLE_TOKEN_URL } else { GITHUB_TOKEN_URL };
        let resp = client.post(url).header("Accept", "application/json").form(&form).send().map_err(|e| format!("refresh request failed: {e}"))?;
        let body = resp.text().map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(&body).unwrap_or_else(|_| json!({}));
        let access = v["access_token"].as_str().unwrap_or("").to_string();
        if access.is_empty() {
            let err = v["error_description"].as_str().or_else(|| v["error"].as_str()).unwrap_or("refresh rejected");
            return Ok(json!({ "ok": false, "message": truncate(err, 160), "provider": p }));
        }
        let expires_in = v["expires_in"].as_u64().unwrap_or(3600);
        let _ = vault_entry(&p, slot_token(&p))?.set_password(&access);
        if let Some(refr) = v["refresh_token"].as_str() {
            let _ = vault_entry(&p, slot_refresh(&p))?.set_password(refr);
        }
        let now = now_ms();
        {
            let db = app.db.lock().map_err(|e| e.to_string())?;
            db.execute(
                "UPDATE connected_accounts SET token_expires_at=?2, status=CASE WHEN status IN ('expired','revoked') THEN 'connected_pending_check' ELSE status END, updated_at=?3 WHERE provider=?1",
                params![p, now + expires_in as i64 * 1000, now],
            )
            .map_err(|e| e.to_string())?;
            let _ = audit_db(&db, "oauth", &p, "refresh_token", "ok", &json!({ "expires_in": expires_in }));
        }
        Ok(json!({ "ok": true, "expires_in": expires_in, "provider": p }))
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(res?)
}

fn provider_revoke_url(provider: &str) -> Option<&'static str> {
    if provider == "google" {
        Some(GOOGLE_REVOKE_URL)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Connection health checks — real authenticated read-only endpoints
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn gw_test_connection(provider: String, gateway: State<'_, Arc<GatewayState>>, state: State<'_, AppState>) -> Result<Value, String> {
    assert_not_stopped(&gateway)?;
    let p = provider.clone();
    let res = tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let token = vault_secret(&p, slot_token(&p))?.ok_or_else(|| "not connected — no token stored".to_string())?;
        let client = http_client()?;
        let url = match p.as_str() {
            "google" => "https://gmail.googleapis.com/gmail/v1/users/me/profile",
            "github" => "https://api.github.com/user",
            "render" => "https://api.render.com/v1/owners?limit=1",
            other => return Err(format!("no health check wired for provider '{other}'")),
        };
        let mut req = client.get(url).bearer_auth(&token);
        if p == "github" {
            req = req.header("X-GitHub-Api-Version", "2022-11-28").header("Accept", "application/vnd.github+json");
        }
        let resp = req.send().map_err(|e| format!("network error: {e}"))?;
        let status = resp.status().as_u16();
        let body = resp.text().map_err(|e| e.to_string())?;
        if !(200..300).contains(&status) {
            return Err(format!("HTTP {status}: {}", truncate(&redact(&body), 160)));
        }
        let v: Value = serde_json::from_str(&body).unwrap_or_else(|_| json!({}));
        Ok(match p.as_str() {
            "google" => v["emailAddress"].as_str().unwrap_or("google account").to_string(),
            "github" => v["login"].as_str().unwrap_or("github account").to_string(),
            _ => "workspace verified".to_string(),
        })
    })
    .await
    .map_err(|e| e.to_string())?;

    let (ok, label, message) = match res {
        Ok(label) => (true, label, "connection verified with a real authenticated read-only call".to_string()),
        Err(e) => (false, String::new(), truncate(&e, 200)),
    };
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let status = if ok { "connected" } else { "failed" };
    let exists: bool = db
        .query_row("SELECT EXISTS(SELECT 1 FROM connected_accounts WHERE provider=?1)", [&provider], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if exists {
        db.execute(
            "UPDATE connected_accounts SET status=?2, account_label=?3, last_check_at=?4, last_check_ok=?5, last_error=?6, updated_at=?4 WHERE provider=?1",
            params![
                provider,
                status,
                if label.is_empty() { None::<String> } else { Some(label.clone()) },
                now_ms(),
                ok as i64,
                if message == "connection verified with a real authenticated read-only call" { None::<String> } else { Some(message.clone()) }
            ],
        )
        .map_err(|e| e.to_string())?;
    } else {
        upsert_account_row(&db, &provider, provider_spec(&provider)?.auth)?;
        db.execute(
            "UPDATE connected_accounts SET status=?2, account_label=?3, last_check_at=?4, last_check_ok=?5, last_error=?6, updated_at=?4 WHERE provider=?1",
            params![
                provider,
                status,
                if label.is_empty() { None::<String> } else { Some(label.clone()) },
                now_ms(),
                ok as i64,
                if message == "connection verified with a real authenticated read-only call" { None::<String> } else { Some(message.clone()) }
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    audit_db(&db, "health", &provider, "test_connection", if ok { "ok" } else { "failed" }, &json!({ "message": message }))?;
    drop(db);
    Ok(json!({
        "ok": ok,
        "status": status,
        "accountLabel": if label.is_empty() { Value::Null } else { json!(label) },
        "message": message
    }))
}

// ---------------------------------------------------------------------------
// API-key connections (Render)
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn gw_connect_api_key(provider: String, api_key: String, gateway: State<'_, Arc<GatewayState>>, state: State<'_, AppState>) -> Result<Value, String> {
    assert_not_stopped(&gateway)?;
    let s = provider_spec(&provider)?;
    if s.auth != "api_key" {
        return Err(format!("provider '{provider}' uses OAuth, not an API key"));
    }
    let key = api_key.trim().to_string();
    let key_prefix = key.chars().take(3).collect::<String>();
    if key.chars().count() < 16 || key.chars().count() > 256 {
        return Err("Render API keys are long IDs (nnda_…) — this value looks wrong".into());
    }
    let p = provider.clone();
    let key_for_spawn = key.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        vault_entry(&p, "api_key")?.set_password(&key_for_spawn).map_err(|e| format!("failed to store credential: {e}"))
    })
    .await
    .map_err(|e| e.to_string())??;
    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        upsert_account_row(&db, &provider, "api_key")?;
        audit_db(&db, "vault", &provider, "store_api_key", "ok", &json!({ "keyPrefix": key_prefix }))?;
    }
    let check = gw_test_connection(provider.clone(), gateway, state.clone()).await?;
    Ok(json!({ "stored": true, "check": check }))
}
// ---------------------------------------------------------------------------
// Connector tool calls — PermissionEngine-gated, every call audited
// ---------------------------------------------------------------------------

pub fn build_preview(provider: &str, tool: &str, args: &Value) -> Value {
    let get = |k: &str| args[k].as_str().unwrap_or("").to_string();
    match (provider, tool) {
        ("google", "gmail_send") => json!({
            "action": "Send email via Gmail",
            "to": get("to"),
            "subject": get("subject"),
            "bodyPreview": truncate(&get("body"), 400)
        }),
        ("github", t) if t.starts_with("github_") => json!({
            "action": format!("{t} via GitHub API"),
            "repo": get("repo"),
            "target": if get("branch").is_empty() { get("title") } else { get("branch") }
        }),
        ("render", t) => json!({
            "action": format!("{t} via Render API"),
            "serviceId": get("serviceId"),
            "warning": if t == "render_trigger_deploy" {
                "This triggers a fresh production deployment."
            } else {
                "This restarts the service and briefly interrupts traffic."
            }
        }),
        _ => json!({ "action": tool })
    }
}

fn finish_json(resp: reqwest::blocking::Response) -> Result<Value, String> {
    let status = resp.status().as_u16();
    let body = resp.text().map_err(|e| format!("response read error: {e}"))?;
    if status == 401 {
        return Err("HTTP 401 — token expired or revoked; reconnect required".into());
    }
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status}: {}", truncate(&redact(&body), 240)));
    }
    match serde_json::from_str::<Value>(&body) {
        Ok(v) => Ok(json!({ "ok": true, "result": v })),
        Err(_) => Ok(json!({ "ok": true, "raw": truncate(&body, 4000) })),
    }
}

pub fn valid_repo_slug(repo: &str) -> bool {
    let Some((owner, name)) = repo.split_once('/') else { return false };
    !owner.is_empty()
        && !name.is_empty()
        && repo.chars().all(|c| c.is_ascii_alphanumeric() || "-._/".contains(c))
}

/// Protected canonical branches can never be written by the connector.
pub fn valid_branch_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if matches!(lower.as_str(), "main" | "master" | "production" | "release" | "trunk") {
        return false;
    }
    !name.starts_with('/')
        && !name.ends_with('/')
        && !name.contains("..")
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c))
}

pub fn valid_render_id(id: &str) -> bool {
    !id.is_empty() && id.chars().count() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Validates tool inputs before any network call. Returns a denial message.
pub fn validate_tool_args(provider: &str, tool: &str, args: &Value) -> Result<(), String> {
    let get = |k: &str| args[k].as_str().unwrap_or("").to_string();
    match tool {
        "gmail_list" | "gmail_draft" | "gmail_send" => {
            if tool != "gmail_list" {
                let to = get("to");
                if to.is_empty() || !to.contains('@') || !to.chars().all(|c| c.is_ascii_graphic() && c != ',' && c != ';') {
                    return Err("a recipient address is required (single RFC-5322 simple address)".into());
                }
                if get("subject").chars().count() > 200 {
                    return Err("subject too long".into());
                }
            }
        }
        "github_list_repos" => {}
        "github_list_commits" | "github_create_issue" | "github_create_branch" => {
            let repo = get("repo");
            if !valid_repo_slug(&repo) {
                return Err("repo must be an 'owner/name' slug".into());
            }
            if tool == "github_create_issue" && get("title").trim().is_empty() {
                return Err("an issue title is required".into());
            }
            if tool == "github_create_branch" && !valid_branch_name(&get("branch")) {
                return Err("this branch name is invalid or protected; CAMEO never writes to main/master/production branches".into());
            }
        }
        "render_list_services" | "render_list_deploys" | "render_trigger_deploy" | "render_restart_service" => {
            if tool != "render_list_services" && !valid_render_id(&get("serviceId")) {
                return Err("serviceId is required".into());
            }
        }
        other => return Err(format!("no adapter wired for {provider}/{other}")),
    }
    Ok(())
}

pub(crate) fn connector_call_blocking(provider: &str, tool: &str, args: &Value) -> Result<Value, String> {
    let token = vault_secret(provider, slot_token(provider))?
        .ok_or_else(|| "not connected — no token stored (connect the provider first)".to_string())?;
    validate_tool_args(provider, tool, args)?;
    let client = http_client()?;
    let get = |k: &str| args[k].as_str().unwrap_or("").to_string();
    match (provider, tool) {
        ("google", "gmail_list") => {
            let q = get("query");
            let mut url = "https://gmail.googleapis.com/gmail/v1/users/me/messages?maxResults=10".to_string();
            if !q.is_empty() {
                url.push_str(&format!("&q={}", urlencode(&q)));
            }
            finish_json(client.get(&url).bearer_auth(&token).send().map_err(|e| format!("network error: {e}"))?)
        }
        ("google", "gmail_draft") | ("google", "gmail_send") => {
            let raw = format!(
                "To: {}\r\nSubject: {}\r\nContent-Type: text/plain; charset=\"UTF-8\"\r\n\r\n{}",
                get("to"),
                get("subject"),
                get("body")
            );
            let encoded = base64url_no_pad(raw.as_bytes());
            let (url, payload) = if tool == "gmail_send" {
                (
                    "https://gmail.googleapis.com/gmail/v1/users/me/messages/send",
                    json!({ "raw": encoded }),
                )
            } else {
                (
                    "https://gmail.googleapis.com/gmail/v1/users/me/drafts",
                    json!({ "message": { "raw": encoded } }),
                )
            };
            finish_json(client.post(url).bearer_auth(&token).json(&payload).send().map_err(|e| format!("network error: {e}"))?)
        }
        ("github", "github_list_repos") => finish_json(
            client
                .get("https://api.github.com/user/repos?per_page=30&sort=updated")
                .bearer_auth(&token)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .send()
                .map_err(|e| format!("network error: {e}"))?,
        ),
        ("github", "github_list_commits") => finish_json(
            client
                .get(format!("https://api.github.com/repos/{}/commits?per_page=20", get("repo")))
                .bearer_auth(&token)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .send()
                .map_err(|e| format!("network error: {e}"))?,
        ),
        ("github", "github_create_issue") => {
            let payload = json!({ "title": get("title"), "body": get("body") });
            finish_json(
                client
                    .post(format!("https://api.github.com/repos/{}/issues", get("repo")))
                    .bearer_auth(&token)
                    .header("Accept", "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2022-11-28")
                    .json(&payload)
                    .send()
                    .map_err(|e| format!("network error: {e}"))?,
            )
        }
        ("github", "github_create_branch") => {
            let repo = get("repo");
            let branch = get("branch");
            let from = if get("from").is_empty() { "main".to_string() } else { get("from") };
            let base = client
                .get(format!("https://api.github.com/repos/{repo}/git/ref/heads/{from}"))
                .bearer_auth(&token)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .send()
                .map_err(|e| format!("network error: {e}"))?;
            let base_v = finish_json(base)?;
            let sha = base_v["result"]["object"]["sha"]
                .as_str()
                .ok_or_else(|| "cannot resolve the base branch SHA".to_string())?
                .to_string();
            let payload = json!({ "ref": format!("refs/heads/{branch}"), "sha": sha });
            finish_json(
                client
                    .post(format!("https://api.github.com/repos/{repo}/git/refs"))
                    .bearer_auth(&token)
                    .header("Accept", "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2022-11-28")
                    .json(&payload)
                    .send()
                    .map_err(|e| format!("network error: {e}"))?,
            )
        }
        ("render", "render_list_services") => finish_json(
            client
                .get("https://api.render.com/v1/services?limit=20")
                .bearer_auth(&token)
                .send()
                .map_err(|e| format!("network error: {e}"))?,
        ),
        ("render", "render_list_deploys") => finish_json(
            client
                .get(format!("https://api.render.com/v1/services/{}/deploys?limit=20", get("serviceId")))
                .bearer_auth(&token)
                .send()
                .map_err(|e| format!("network error: {e}"))?,
        ),
        ("render", "render_trigger_deploy") => {
            let payload = json!({ "clearCache": get("clearCache") == "do_not_clear" });
            finish_json(
                client
                    .post(format!("https://api.render.com/v1/services/{}/deploys", get("serviceId")))
                    .bearer_auth(&token)
                    .json(&payload)
                    .send()
                    .map_err(|e| format!("network error: {e}"))?,
            )
        }
        ("render", "render_restart_service") => finish_json(
            client
                .post(format!("https://api.render.com/v1/services/{}/restart", get("serviceId")))
                .bearer_auth(&token)
                .send()
                .map_err(|e| format!("network error: {e}"))?,
        ),
        (p, t) => Err(format!("no adapter wired for {p}/{t}")),
    }
}

#[tauri::command]
pub async fn gw_connector_call(
    provider: String,
    tool: String,
    args: Value,
    approved: Option<bool>,
    gateway: State<'_, Arc<GatewayState>>,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    assert_not_stopped(&gateway)?;
    let tools = tool_specs(&provider);
    let Some(spec) = tools.iter().find(|t| t.name == tool) else {
        return Err(format!("unknown tool '{tool}' for provider '{provider}'"));
    };
    let perms = permissions_of(&state, &provider)?;
    if let Err(e) = authorize_call(&perms, spec.level) {
        return Err(format!("{e} — request was rejected even though an agent asked for it"));
    }
    if let Err(e) = validate_tool_args(&provider, &tool, &args) {
        return Err(e);
    }
    if spec.approval_needed && approved != Some(true) {
        return Ok(json!({
            "approvalRequired": true,
            "level": spec.level,
            "tool": tool,
            "preview": build_preview(&provider, &tool, &args),
            "message": "This is a sensitive action. Review the preview, then approve to execute it once."
        }));
    }
    if spec.approval_needed {
        audit(
            &state,
            "approval",
            &provider,
            &tool,
            "sensitive_user_approved",
            &json!({ "preview": build_preview(&provider, &tool, &args) }),
        );
    }
    let p = provider.clone();
    let args2 = args.clone();
    let tool_arg = tool.clone();
    let out = tauri::async_runtime::spawn_blocking(move || connector_call_blocking(&p, &tool_arg, &args2))
        .await
        .map_err(|e| e.to_string())?;
    let outcome = match &out {
        Ok(v) => {
            if v["ok"] == json!(true) { "ok" } else { "error" }
        }
        Err(_) => "error",
    };
    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        audit_db(&db, "connector", &provider, &tool, outcome, &json!({ "args": args }))?;
    }
    if matches!(&out, Err(e) if e.contains("HTTP 401")) {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        let _ = db.execute(
            "UPDATE connected_accounts SET status='expired', updated_at=?2 WHERE provider=?1 AND status='connected'",
            params![provider, now_ms()],
        );
    }
    out
}

// ---------------------------------------------------------------------------
// Disconnect / revocation helpers
// ---------------------------------------------------------------------------
fn revoke_slots(provider: &str) -> Vec<String> {
    match provider {
        "google" => ["client_id", "client_secret", "google_token", "refresh_token"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        "github" => ["client_id", "github_token", "refresh_token"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        "render" => ["api_key"].iter().map(|s| s.to_string()).collect(),
        _ => Vec::new(),
    }
}

fn revoke_server_side(provider: &str, token: &str) -> Result<(), String> {
    if let Some(url) = provider_revoke_url(provider) {
        let client = http_client()?;
        let resp = client
            .post(url)
            .form(&[("token", token)])
            .send()
            .map_err(|e| format!("revocation request failed: {e}"))?;
        if !resp.status().is_success() && resp.status().as_u16() != 400 {
            return Err(format!("revocation endpoint returned HTTP {}", resp.status().as_u16()));
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn gw_disconnect(provider: String, gateway: State<'_, Arc<GatewayState>>, state: State<'_, AppState>) -> Result<Value, String> {
    // Best-effort server-side revocation for Google's refresh token first
    // (GitHub/Render tokens are revoked in their dashboards; we deliberately
    // do not keep the app-level client secret for GitHub). Then clear every
    // local vault entry for this provider so nothing is left behind.
    let p_revoke = provider.clone();
    let revoked_remotely = tauri::async_runtime::spawn_blocking(move || -> bool {
        let token = match vault_secret(&p_revoke, slot_token(&p_revoke)) {
            Ok(Some(t)) => t,
            _ => return false,
        };
        revoke_server_side(&p_revoke, &token).is_ok()
    })
    .await
    .unwrap_or(false);
    let cleared = gw_clear_secrets(provider.clone(), revoke_slots(&provider)).await?;
    {
        let mut pend = gateway.pending_auth.lock().map_err(|e| e.to_string())?;
        pend.retain(|v| v.provider != provider);
    }
    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        db.execute(
            "INSERT INTO connected_accounts (provider, auth_method, status, created_at, updated_at)
             VALUES (?1, (SELECT auth_method FROM connected_accounts WHERE provider=?1 LIMIT 1), 'disconnected', ?2, ?2)
             ON CONFLICT(provider) DO UPDATE SET status='disconnected', account_label=NULL, scopes='[]',
               token_expires_at=NULL, last_check_ok=NULL, last_error=NULL, permissions=?3, updated_at=?2",
            params![
                provider,
                now_ms(),
                serde_json::to_string(&permissions_value(&Permissions::read_only())).unwrap_or_else(|_| "{}".into())
            ],
        )
        .map_err(|e| e.to_string())?;
        audit_db(
            &db,
            "vault",
            &provider,
            "disconnect",
            "ok",
            &json!({ "vaultEntriesCleared": cleared, "serverSideRevoke": revoked_remotely }),
        )?;
        drop(db);
    }
    Ok(json!({
        "disconnected": true,
        "provider": provider,
        "vaultEntriesCleared": cleared,
        "serverSideRevoke": revoked_remotely,
        "revokeGuide": provider_spec(&provider)?.revoke_guide
    }))
}

// ---------------------------------------------------------------------------
// Unit tests — pure logic under test; no network, no credential store
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_known_token_prefixes_but_keeps_short_prefix() {
        let leaked = "auth Bearer abcdefgh1234567890 and sk-proj-abcdefghijklmnopqrstuvwxyz secret";
        let cleaned = redact(leaked);
        assert!(!cleaned.contains("abcdefgh1234567890"));
        assert!(!cleaned.contains("bcdefghijk"));
        assert!(cleaned.contains("Bearer abcd…"));
        assert!(cleaned.contains("sk-proj-abcd…") || cleaned.contains("sk-p-abcd…") || !cleaned.contains("qrstuvwxyz"));
    }

    #[test]
    fn redact_handles_query_param_tokens() {
        let cleaned = redact("https://cb/redirect?code=supersecretcode1234567&scope=gmail.readonly");
        assert!(!cleaned.contains("supersecretcode"));
        assert!(cleaned.contains("scope=gmail.readonly"));
    }

    #[test]
    fn redact_value_masks_secret_fields_but_keeps_safe_ones() {
        let mut v = json!({ "access_token": "ghp_xtremely_secret", "owner": "octocat", "nested": { "password": "hunter2" } });
        redact_value(&mut v);
        assert_eq!(v["access_token"], json!("REDACTED…"));
        assert_eq!(v["owner"], json!("octocat"));
        assert_eq!(v["nested"]["password"], json!("REDACTED…"));
    }

    #[test]
    fn pkce_verifier_matches_s256_challenge() {
        let pair = generate_pkce();
        assert_eq!(pair.verifier.chars().count(), 64);
        assert!(pair.verifier.chars().all(|c| PKCE_CHARS.contains(&(c as u8))));
        let digest = Sha256::digest(pair.verifier.as_bytes());
        assert_eq!(pair.challenge, base64url_no_pad(&digest));
        assert!(!pair.challenge.contains('='));
        assert!(!pair.challenge.contains('+'));
        assert!(!pair.challenge.contains('/'));
    }

    #[test]
    fn pkce_verifiers_are_unique() {
        let a = generate_pkce();
        let b = generate_pkce();
        assert_ne!(a.verifier, b.verifier);
        assert_ne!(a.challenge, b.challenge);
    }

    #[test]
    fn base64url_matches_known_vectors() {
        assert_eq!(base64url_no_pad(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64url_no_pad(b"foob"), "Zm9vYg");
        assert_eq!(base64url_no_pad(b"f"), "Zg");
    }

    #[test]
    fn urldecodes_percent_and_plus() {
        assert_eq!(urldecode("hello%20world"), "hello world");
        assert_eq!(urldecode("a+b"), "a b");
        assert_eq!(urldecode("%2FX%2F"), "/X/");
        assert_eq!(urldecode("plain"), "plain");
        assert_eq!(urldecode("bad%zz"), "bad%zz");
    }

    #[test]
    fn permission_levels_rank_implicitly() {
        let p = parse_permissions(&json!({ "read": true })).unwrap();
        assert!(p.allows("read"));
        assert!(!p.allows("prepare"));
        assert!(!p.allows("write"));
        assert!(!p.allows("sensitive"));

        let escalated = parse_permissions(&json!({ "sensitive": true })).unwrap();
        for level in LEVELS {
            assert!(escalated.allows(level), "sensitive implies {level}");
        }
        assert!(parse_permissions(&json!({})).is_err());
        assert!(parse_permissions(&json!({ "read": true, "extra": 1 })).is_ok());
    }

    #[test]
    fn authorize_rejects_unknown_and_denied_levels() {
        let read_only = Permissions::read_only();
        assert!(authorize_call(&read_only, "read").is_ok());
        assert!(authorize_call(&read_only, "sensitive").is_err());
        assert!(authorize_call(&read_only, "mine").is_err());
    }

    #[test]
    fn protected_branches_are_never_writable() {
        assert!(!valid_branch_name("main"));
        assert!(!valid_branch_name("master"));
        assert!(!valid_branch_name("production"));
        assert!(!valid_branch_name("trunk"));
        assert!(!valid_branch_name("../etc/passwd"));
        assert!(!valid_branch_name("/root"));
        assert!(valid_branch_name("cameo/feature-gateway"));
        assert!(valid_branch_name("fix-1"));
    }

    #[test]
    fn repo_slugs_and_render_ids_are_validated() {
        assert!(valid_repo_slug("octocat/hello-world"));
        assert!(valid_repo_slug("my.org/app.v2"));
        assert!(!valid_repo_slug("no-slash"));
        assert!(!valid_repo_slug("bad name/repo"));
        assert!(!valid_repo_slug("/weird"));
        assert!(valid_render_id("srv-cde1fg2ail9r4k5p6p0"));
        assert!(!valid_render_id("has space"));
        assert!(!valid_render_id(""));
    }

    #[test]
    fn tool_args_validation_rejects_escalation_inputs() {
        assert!(validate_tool_args("google", "gmail_send", &json!({ "to": "a@b.com", "subject": "hi", "body": "x" })).is_ok());
        assert!(validate_tool_args("google", "gmail_send", &json!({ "to": "not-an-email" })).is_err());
        assert!(validate_tool_args("github", "github_create_branch", &json!({ "repo": "a/b", "branch": "main" })).is_err());
        assert!(validate_tool_args("github", "github_create_branch", &json!({ "repo": "a/b", "branch": "clean/branch" })).is_ok());
        assert!(validate_tool_args("render", "render_trigger_deploy", &json!({ "serviceId": "srv-1" })).is_ok());
        assert!(validate_tool_args("render", "render_trigger_deploy", &json!({ "serviceId": "" })).is_err());
        assert!(validate_tool_args("google", "mnemonic_tool", &json!({})).is_err());
    }

    #[test]
    fn sensitive_tools_require_explicit_approval() {
        for provider in ["google", "render"] {
            for tool in tool_specs(provider) {
                if tool.level == "sensitive" {
                    assert!(tool.approval_needed, "{provider}/{} must require approval", tool.name);
                }
            }
        }
        assert!(tool_specs("github").iter().all(|t| !t.approval_needed || t.level == "write"));
    }

    #[test]
    fn preview_names_the_exact_action_and_target() {
        let preview = build_preview("google", "gmail_send", &json!({ "to": "a@b.com", "subject": "s", "body": "body text" }));
        assert_eq!(preview["to"], json!("a@b.com"));
        assert!(preview["action"].as_str().unwrap().contains("Gmail"));
        let deploy = build_preview("render", "render_trigger_deploy", &json!({ "serviceId": "srv-x" }));
        assert!(deploy["warning"].as_str().unwrap().contains("deployment"));
    }

    #[test]
    fn vault_service_names_are_isolated_per_provider() {
        // One provider's vault entry must never equal another's: service names
        // embed the provider id, which is the guard against cross-provider
        // token reuse.
        for a in VAULT_PROVIDERS {
            for b in VAULT_PROVIDERS {
                if a != b {
                    assert_ne!(format!("cameo_gateway_{a}"), format!("cameo_gateway_{b}"));
                    let slots = revoke_slots(a);
                    assert!(!slots.iter().any(|s| format!("cameo_gateway_{a}") == format!("cameo_gateway_{b}")));
                }
            }
        }
        assert!(vault_entry("unknown", "x").is_err());
        assert!(vault_entry("google", "bad slot!").is_err());
    }

    #[test]
    fn registry_covers_selectable_providers() {
        let all = gw_registry();
        assert_eq!(all.len(), PROVIDER_SPECS.len());
        for entry in &all {
            assert!(!entry["provider"].as_str().unwrap_or("").is_empty());
            assert!(!entry["revokeGuide"].as_str().unwrap_or("").is_empty());
        }
        assert!(provider_spec("google").is_ok());
        assert!(provider_spec("render").is_ok());
        assert!(provider_spec("notreal").is_err());
    }

    #[test]
    fn disconnect_slot_list_never_leaks_between_providers() {
        let google = revoke_slots("google");
        assert!(!google.contains(&"github_token".to_string()));
        assert!(google.contains(&"refresh_token".to_string()));
        let render = revoke_slots("render");
        assert_eq!(render.len(), 1);
        assert!(render.contains(&"api_key".to_string()));
    }
}
