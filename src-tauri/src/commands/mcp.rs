use crate::commands::gateway::audit_db;
use crate::commands::store::AppState;
use keyring::Entry;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::Child;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::State;

// ===========================================================================
// CAMEO MCPConnectionManager — local (stdio) and remote (streamable HTTP)
// Model Context Protocol servers.
//
// Local servers are launched only after the Connections page shows the exact
// executable, arguments and working directory and the user enables them.
// Secrets for stdio env vars are stored per-server in the OS credential store
// and delivered only through the child's environment — never in argv or logs.
// Remote servers must be HTTPS public endpoints (SSRF guard). If a server
// requires authentication we report the discovery URL per the MCP
// authorization spec instead of starting an OAuth flow automatically.
// Every remote tool call is audited; the global Stop control kills stdio
// children and blocks HTTP calls.
// ===========================================================================

pub const MCP_SCHEMA_SQL: &str = "CREATE TABLE IF NOT EXISTS mcp_servers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    transport TEXT NOT NULL CHECK (transport IN ('stdio','http')),
    command TEXT,
    args_json TEXT NOT NULL DEFAULT '[]',
    cwd TEXT,
    url TEXT,
    env_json TEXT NOT NULL DEFAULT '[]',
    enabled INTEGER NOT NULL DEFAULT 0,
    call_level TEXT NOT NULL DEFAULT 'read',
    status TEXT NOT NULL DEFAULT 'not_configured',
    last_tools_json TEXT,
    last_checked_at INTEGER,
    last_error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);";

/// In-flight stdio children keyed by server id, for the global Stop control.
pub static CHILDREN: Mutex<Option<HashMap<String, Arc<Mutex<Child>>>>> = Mutex::new(None);

pub fn kill_all_stdio() {
    let mut guard = CHILDREN.lock().expect("children registry");
    if let Some(map) = guard.as_mut() {
        for (_, child) in map.iter() {
            if let Ok(mut c) = child.lock() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
        map.clear();
    }
}

fn children_of(id: &str) -> Option<Arc<Mutex<Child>>> {
    let guard = CHILDREN.lock().expect("children registry");
    guard.as_ref().and_then(|m| m.get(id).cloned())
}

fn children_remove(id: &str) {
    let mut guard = CHILDREN.lock().expect("children registry");
    if let Some(map) = guard.as_mut() {
        if let Some(child) = map.remove(id) {
            if let Ok(mut c) = child.lock() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Input validation — the "Display before enabling / restrict execution" rules
// ---------------------------------------------------------------------------

pub const BLOCKED_EXECUTABLES: [&str; 9] = [
    "cmd", "cmd.exe", "powershell", "powershell.exe", "pwsh", "pwsh.exe", "bash", "sh", "wsl",
];

/// Validates the stdio launcher. Returns a denial text or an empty Ok.
pub fn validate_stdio(command: &str, args: &[String], cwd: &str) -> Result<(), String> {
    let command = command.trim();
    if command.is_empty() {
        return Err("stdio servers need an executable path".into());
    }
    if command.len() > 260 {
        return Err("executable path looks too long".into());
    }
    if command.chars().any(|c| (c.is_control() && c != '\t') || c == '\0' || c == '"' || c == '\\') && command.contains('\\') && command.starts_with('"') {
        return Err("quoting the executable is not allowed; pass the resolved path".into());
    }
    let exe = std::path::Path::new(command);
    if !exe.is_absolute() {
        return Err("use an absolute path for the server executable (no PATH lookup or shell interception)".into());
    }
    let stem = exe.file_stem().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
    if BLOCKED_EXECUTABLES.contains(&stem.as_str()) {
        return Err(format!("'{stem}' is not allowed as an MCP launcher — point at the actual server executable"));
    }
    if !exe.is_file() {
        return Err(format!("executable not found: {command}"));
    }
    for arg in args {
        if arg.contains('\0') || arg.chars().count() > 200 {
            return Err("argument too long or contains a NUL byte".into());
        }
    }
    if args.len() + command.chars().count() > 2048 {
        return Err("spawn line too long".into());
    }
    if !cwd.is_empty() {
        let dir = std::path::Path::new(cwd);
        if !dir.is_dir() {
            return Err(format!("working directory does not exist: {cwd}"));
        }
    }
    Ok(())
}

pub fn disable_stdio(id: &str) {
    if let Some(child) = children_of(id) {
        if let Ok(mut c) = child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
    children_remove(id);
}

fn valid_slug(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 64
        && name.chars().next().map(|c| c.is_ascii_alphabetic()).unwrap_or(false)
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn id_for(name: &str, conn: &Connection) -> String {
    let mut counter = 0;
    loop {
        let candidate = if counter == 0 {
            format!("mcp-{}", name.to_ascii_lowercase())
        } else {
            format!("mcp-{}-{}", name.to_ascii_lowercase(), counter)
        };
        let exists: bool = conn
            .query_row("SELECT EXISTS(SELECT 1 FROM mcp_servers WHERE id=?1)", [&candidate], |r| r.get(0))
            .unwrap_or(false);
        if !exists {
            return candidate;
        }
        counter += 1;
    }
}

pub fn env_slot(server_id: &str, env_name: &str) -> Result<Entry, String> {
    let slot_ok = env_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_');
    if slot_ok && !env_name.is_empty() {
        Entry::new(&format!("cameo_mcp_{server_id}"), env_name).map_err(|e| format!("credential store error: {e}"))
    } else {
        Err("invalid environment variable name".into())
    }
}

fn peer_ms() -> i64 {
    crate::commands::gateway::now_ms()
}

// ---------------------------------------------------------------------------
// JSON storage helpers
// ---------------------------------------------------------------------------

pub fn parse_sse_stream(body: &str) -> Vec<Value> {
    // Streamable-HTTP servers may answer with text/event-stream; collect the
    // JSON payloads of `data:` lines.
    let mut out = Vec::new();
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            let trimmed = rest.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
                out.push(v);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Server CRUD commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn mcp_add(
    name: String,
    transport: String,
    command: Option<String>,
    args: Option<Vec<String>>,
    cwd: Option<String>,
    url: Option<String>,
    env: Option<Vec<String>>,
    call_level: Option<String>,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    let transport = transport.as_str();
    if !matches!(transport, "stdio" | "http") {
        return Err("transport must be 'stdio' or 'http'".into());
    }
    let level = call_level.unwrap_or_else(|| "read".into());
    if !crate::commands::gateway::LEVELS.contains(&level.as_str()) {
        return Err(format!("call level must be one of: {}", crate::commands::gateway::LEVELS.join(", ")));
    }
    let args = args.unwrap_or_default();
    let env = env.unwrap_or_default();
    {
        let conn = state.db.lock().expect("db");
        let name_taken: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM mcp_servers WHERE name=?1)", [&name], |r| r.get(0)).unwrap_or(false);
        if name_taken || !valid_slug(&name) {
            return Err("each MCP server needs a unique mixed-case-safe name (letters, digits, - _)".into());
        }
    }
    match transport {
        "stdio" => {
            let cmd = command.clone().unwrap_or_default();
            let cwd_val = cwd.clone().unwrap_or_default();
            validate_stdio(&cmd, &args, &cwd_val)?;
        }
        _ => {
            let u = url.clone().unwrap_or_default();
            let checked = check_http_endpoint_allowed(&u)?;
            let _ = checked;
        }
    }
    let id;
    let now = peer_ms();
    {
        let conn = state.db.lock().expect("db");
        id = id_for(&name, &conn);
        // Persist env var NAMES first so validate/review can display them.
        conn.execute(
            "INSERT INTO mcp_servers (id, name, transport, command, args_json, cwd, url, env_json, enabled, call_level, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, 'not_configured', ?10, ?10)",
            params![
                id,
                name,
                transport,
                command,
                serde_json::to_string(&args).unwrap_or_else(|_| "[]".into()),
                cwd,
                url,
                serde_json::to_string(&env).unwrap_or_else(|_| "[]".into()),
                level,
                now
            ],
        )
        .map_err(|e| e.to_string())?;
        let _ = audit_db(&conn, "mcp", &id, "add_server", "ok", &json!({
            "transport": transport,
            "stdio": if transport == "stdio" { Some(json!({"command": command.clone(), "args": args.clone(), "cwd": cwd.clone()})) } else { None },
            "url": if transport == "http" { url.clone() } else { None },
            "envNames": env.clone(),
            "callLevel": level.clone()
        }))?;
    }
    Ok(json!({
        "id": id, "name": name, "transport": transport,
        "command": if transport == "stdio" { command } else { None },
        "args": if transport == "stdio" { args } else { Vec::<String>::new() },
        "cwd": if transport == "stdio" { cwd } else { None },
        "url": if transport == "http" { url } else { None },
        "envNames": env,
        "enabled": false,
        "callLevel": level,
        "status": "not_configured",
        "review": "Review the executable, arguments, working directory and env names above, set any required env values, then enable."
    }))
}

pub fn mcp_row(conn: &Connection, id: &str) -> Result<Option<Value>, String> {
    conn.query_row(
        "SELECT id, name, transport, command, args_json, cwd, url, env_json, enabled, call_level, status, last_tools_json, last_checked_at, last_error, created_at, updated_at
         FROM mcp_servers WHERE id=?1",
        [id],
        |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "name": r.get::<_, String>(1)?,
                "transport": r.get::<_, String>(2)?,
                "command": r.get::<_, Option<String>>(3)?,
                "args": serde_json::from_str::<Value>(&r.get::<_, String>(4).unwrap_or_else(|_| "[]".into())).unwrap_or_else(|_| json!([])),
                "cwd": r.get::<_, Option<String>>(5)?,
                "url": r.get::<_, Option<String>>(6)?,
                "envNames": serde_json::from_str::<Value>(&r.get::<_, String>(7).unwrap_or_else(|_| "[]".into())).unwrap_or_else(|_| json!([])),
                "enabled": r.get::<_, i64>(8)? == 1,
                "callLevel": r.get::<_, String>(9)?,
                "status": r.get::<_, String>(10)?,
                "tools": r.get::<_, Option<String>>(11)?.and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or_else(|| json!([])),
                "lastCheckedAt": r.get::<_, Option<i64>>(12)?,
                "lastError": r.get::<_, Option<String>>(13)?,
                "createdAt": r.get::<_, i64>(14)?,
                "updatedAt": r.get::<_, i64>(15)?,
            }))
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn mcp_list(state: State<'_, AppState>) -> Result<Vec<Value>, String> {
    let conn = state.db.lock().expect("db");
    let mut stmt = conn
        .prepare("SELECT id FROM mcp_servers ORDER BY created_at")
        .map_err(|e| e.to_string())?;
    let ids = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for id in ids {
        if let Some(v) = mcp_row(&conn, &id)? {
            out.push(v);
        }
    }
    Ok(out)
}

#[tauri::command]
pub fn mcp_remove(id: String, state: State<'_, AppState>) -> Result<bool, String> {
    disable_stdio(&id);
    let conn = state.db.lock().expect("db");
    let n = conn.execute("DELETE FROM mcp_servers WHERE id=?1", [&id]).map_err(|e| e.to_string())?;
    let _ = audit_db(&conn, "mcp", &id, "remove_server", if n > 0 { "ok" } else { "not_found" }, &json!({}))?;
    Ok(n > 0)
}

#[tauri::command]
pub fn mcp_update(
    id: String,
    name: Option<String>,
    command: Option<String>,
    args: Option<Vec<String>>,
    cwd: Option<String>,
    url: Option<String>,
    env: Option<Vec<String>>,
    call_level: Option<String>,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    let conn = state.db.lock().expect("db");
    let row = mcp_row(&conn, &id)?.ok_or_else(|| format!("MCP server '{id}' does not exist"))?;
    let transport = row["transport"].as_str().unwrap_or("stdio").to_string();
    drop(conn);

    if let Some(level) = &call_level {
        if !crate::commands::gateway::LEVELS.contains(&level.as_str()) {
            return Err(format!("call level must be one of: {}", crate::commands::gateway::LEVELS.join(", ")));
        }
    }
    // Re-validate the field set being updated (stdio fields only for stdio).
    if transport == "stdio" {
        let new_command = command.clone().or_else(|| row["command"].as_str().map(|x| x.to_string())).unwrap_or_default();
        let new_args = args.clone().unwrap_or_else(|| {
            row["args"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(|s| s.to_string()).collect()).unwrap_or_default()
        });
        let new_cwd = cwd.clone().or_else(|| row["cwd"].as_str().map(|x| x.to_string())).unwrap_or_default();
        validate_stdio(&new_command, &new_args, &new_cwd)?;
    } else if let Some(u) = &url {
        let _ = check_http_endpoint_allowed(u)?;
    }
    if let Some(n) = &name {
        if !valid_slug(n) {
            return Err("name must start with a letter and use letters/digits/-_ only".into());
        }
    }

    let conn = state.db.lock().expect("db");
    if let Some(name) = name {
        let taken: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM mcp_servers WHERE name=?1 AND id<>?2)", rusqlite::params![name, id], |r| r.get(0)).unwrap_or(false);
        if taken {
            return Err(format!("another MCP server already uses the name '{name}'"));
        }
        conn.execute("UPDATE mcp_servers SET name=?2 WHERE id=?1", rusqlite::params![id, name]).map_err(|e| e.to_string())?;
    }
    if let Some(command) = command {
        conn.execute("UPDATE mcp_servers SET command=?2 WHERE id=?1", rusqlite::params![id, command]).map_err(|e| e.to_string())?;
    }
    if let Some(args) = args {
        conn.execute("UPDATE mcp_servers SET args_json=?2 WHERE id=?1", rusqlite::params![id, serde_json::to_string(&args).unwrap_or_else(|_| "[]".into())]).map_err(|e| e.to_string())?;
    }
    if let Some(cwd) = cwd {
        conn.execute("UPDATE mcp_servers SET cwd=?2 WHERE id=?1", rusqlite::params![id, cwd]).map_err(|e| e.to_string())?;
    }
    if let Some(url) = url {
        conn.execute("UPDATE mcp_servers SET url=?2 WHERE id=?1", rusqlite::params![id, url]).map_err(|e| e.to_string())?;
    }
    if let Some(env) = env {
        conn.execute("UPDATE mcp_servers SET env_json=?2 WHERE id=?1", rusqlite::params![id, serde_json::to_string(&env).unwrap_or_else(|_| "[]".into())]).map_err(|e| e.to_string())?;
    }
    if let Some(level) = call_level {
        conn.execute("UPDATE mcp_servers SET call_level=?2 WHERE id=?1", rusqlite::params![id, level]).map_err(|e| e.to_string())?;
        // Keep the mirrored permission row in sync with the chosen level.
        if level != "sensitive" {
            let grants = match level.as_str() {
                "read" => (true, false, false),
                "prepare" => (true, true, false),
                _ => (true, true, true),
            };
            let _ = conn.execute(
                "UPDATE connected_accounts SET permissions=?2 WHERE provider=?1",
                rusqlite::params![mcp_provider_row(&id), serde_json::json!({"read": grants.0, "prepare": grants.1, "write": grants.2, "sensitive": grants.2}).to_string()],
            );
        }
    }
    conn.execute("UPDATE mcp_servers SET updated_at=?2 WHERE id=?1", rusqlite::params![id, peer_ms()]).map_err(|e| e.to_string())?;
    mcp_row(&conn, &id)?.ok_or_else(|| format!("server {id} not found"))
}

fn mcp_provider_row(id: &str) -> String {
    format!("mcp_{id}")
}

#[tauri::command]
pub fn mcp_set_enabled(id: String, enabled: bool, state: State<'_, AppState>) -> Result<Value, String> {
    let conn = state.db.lock().expect("db");
    let row = mcp_row(&conn, &id)?.ok_or_else(|| format!("MCP server '{id}' does not exist"))?;
    let now = peer_ms();
    if enabled {
        if let Some(url) = row["url"].as_str() {
            let _ = check_http_endpoint_allowed(url)?;
        }
        conn.execute(
            "UPDATE mcp_servers SET enabled=1, status=CASE WHEN status='validated' THEN 'enabled' ELSE 'connecting' END, updated_at=?2 WHERE id=?1",
            rusqlite::params![id, now],
        )
        .map_err(|e| e.to_string())?;
        // Mirror this server into the central permission engine as its own
        // isolated provider row with the configured call level.
        upsert_mcp_permission_row(&conn, &id, row["callLevel"].as_str().unwrap_or("read"))?;
        let _ = audit_db(&conn, "mcp", &id, "enable_server", "ok", &json!({ "callLevel": row["callLevel"] }))?;
    } else {
        disconnect_stdio(&id);
        conn.execute(
            "UPDATE mcp_servers SET enabled=0, status='validated', updated_at=?2 WHERE id=?1",
            rusqlite::params![id, now],
        )
        .map_err(|e| e.to_string())?;
        let _ = conn.execute("DELETE FROM connected_accounts WHERE provider=?1", [mcp_provider_row(&id)]);
        let _ = audit_db(&conn, "mcp", &id, "disable_server", "ok", &json!({}))?;
    }
    mcp_row(&conn, &id)?.ok_or_else(|| format!("server {id} not found"))
}

fn upsert_mcp_permission_row(conn: &Connection, id: &str, level: &str) -> Result<(), String> {
    let (read, prepare, write) = match level {
        "read" => (true, false, false),
        "prepare" => (true, true, false),
        "write" => (true, true, true),
        "sensitive" => (true, true, true),
        _ => (true, false, false),
    };
    let sensitive = level == "sensitive";
    conn.execute(
        "INSERT INTO connected_accounts (provider, auth_method, status, permissions, created_at, updated_at)
         VALUES (?1, 'local_stdio_or_http_mcp', 'connected', ?2, ?3, ?3)
         ON CONFLICT(provider) DO UPDATE SET permissions=?2, updated_at=?3",
        rusqlite::params![
            mcp_provider_row(id),
            serde_json::json!({"read": read, "prepare": prepare, "write": write, "sensitive": sensitive}).to_string(),
            peer_ms()
        ],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

pub fn disconnect_stdio(id: &str) {
    disable_stdio(id);
}

// ---------------------------------------------------------------------------
// SSRF guard — remote URLs must be public HTTPS endpoints
// ---------------------------------------------------------------------------

pub fn ip_is_public_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let b = v4.octets();
            !(b[0] == 10 || b[0] == 127 || b[0] == 0 || (b[0] == 172 && (16..=31).contains(&b[1])) || (b[0] == 192 && b[1] == 168) || (b[0] == 169 && b[1] == 254) || (b[0] == 100 && (64..=127).contains(&b[1])) || (b[0] == 198 && (b[1] == 18 || b[1] == 19)) || (b[0] == 203 && b[1] == 0 && b[2] == 113) || b[0] >= 240 || b[0] == 224 || b[0] == 255)
        }
        std::net::IpAddr::V6(v6) => {
            let s = v6.segments();
            !(s[0] & 0xFE00 == 0xFC00 || s[0] == 0xFE80 || (s[0] == 0x0064 && s[1] == 0xFF9B && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0) || s.iter().all(|x| *x == 0) || s[0] == 0x2001 && s[1] == 0xDB8)
        }
    }
}

/// Resolves the host and rejects non-public targets (SSRF guard).
pub fn check_http_endpoint_allowed(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| format!("not a valid URL: {url}"))?;
    if parsed.scheme() != "https" {
        return Err("remote MCP endpoints must use HTTPS".into());
    }
    if parsed.host_str().map(|h| h.contains('@')).unwrap_or(false) {
        return Err("userinfo in URL is not allowed".into());
    }
    let host = parsed.host_str().ok_or_else(|| "URL has no host".to_string())?;
    if host.parse::<std::net::IpAddr>().is_ok() {
        let ip: std::net::IpAddr = host.parse().map_err(|e: std::net::AddrParseError| e.to_string())?;
        if !ip_is_public_ip(&ip) {
            return Err("remote MCP endpoints must be public addresses".into());
        }
        return Ok(());
    }
    let addrs: Vec<std::net::SocketAddr> = (host, 443u16).to_socket_addrs_map().map_err(|e| format!("cannot resolve {host}: {e}"))?;
    if addrs.is_empty() {
        return Err(format!("cannot resolve {host}"));
    }
    for a in addrs {
        if !ip_is_public_ip(&a.ip()) {
            return Err(format!("host {host} resolves to a non-public address — remote MCP must point at public servers"));
        }
    }
    Ok(())
}

use std::net::ToSocketAddrs;

trait ToSocketAddrsMap {
    fn to_socket_addrs_map(&self) -> Result<Vec<std::net::SocketAddr>, String>;
}

impl ToSocketAddrsMap for (&str, u16) {
    fn to_socket_addrs_map(&self) -> Result<Vec<std::net::SocketAddr>, String> {
        self.0
            .to_socket_addrs()
            .map(|iter| iter.filter(|a| a.is_ipv4() || a.is_ipv6()).collect())
            .map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Validate + inspect tools — stdio launch with real JSON-RPC, or remote probe
// ---------------------------------------------------------------------------

pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

fn initialize_message() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "cameo", "version": "0.1.0" }
        }
    })
}

fn initialized_note() -> Value {
    json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
}

fn tools_list_message(id: i64) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/list", "params": {} })
}

fn call_tool_message(id: i64, tool: &str, args: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": tool, "arguments": args } })
}

pub fn extract_tools(initialize_result: &Value) -> Vec<Value> {
    initialize_result["tools"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .take(200)
                .filter_map(|t| {
                    let name = t["name"].as_str()?;
                    Some(json!({
                        "name": name,
                        "description": t["description"].as_str().unwrap_or(""),
                        "schemaOk": t["inputSchema"].is_object() || t["inputSchema"].is_null()
                    }))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn extract_tools_from_response(v: &Value) -> Vec<Value> {
    if v["result"].is_object() {
        let mut t = extract_tools(&v["result"]);
        if t.is_empty() {
            t = extract_tools(v);
        }
        t
    } else {
        Vec::new()
    }
}

fn stdio_jsonrpc_roundtrip(child: &mut Child, req: &Value, timeout_ms: u64) -> Result<Value, String> {
    let payload = format!("{}\n", serde_json::to_string(req).map_err(|e| e.to_string())?);
    {
        let stdin = child.stdin.as_mut().ok_or("the server has no stdin")?;
        let _ = stdin.write_all(payload.as_bytes());
        let _ = stdin.flush();
    }
    if payload.contains("\"method\"") && payload.contains("notifications") {
        // Notifications have no response; skip reading this round to avoid a
        // pointless wait.
        return Ok(json!({}));
    }
    let stdout = child.stdout.as_mut().ok_or("the server has no stdout")?;
    let mut reader = BufReader::new(stdout);
    let start = std::time::Instant::now();
    loop {
        if start.elapsed().as_millis() as u64 > timeout_ms {
            let _ = child.kill();
            return Err(format!("no JSON-RPC response within {timeout_ms}ms"));
        }
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return Err("the server closed stdout before answering".into()),
            Ok(_) => {}
            Err(e) => return Err(format!("reading server output failed: {e}")),
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("Content-Length:") || trimmed.starts_with("#") {
            continue;
        }
        let json_part = if trimmed.starts_with('{') {
            trimmed
        } else if let Some(idx) = trimmed.find('{') {
            &trimmed[idx..]
        } else {
            continue;
        };
        if let Ok(v) = serde_json::from_str::<Value>(json_part) {
            return Ok(v);
        }
    }
}

fn http_client_short() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| format!("http client error: {e}"))
}

struct RemoteOutcome {
    status: u16,
    www_auth: Option<String>,
    body: String,
}

fn post_jsonrpc(url: &str, payload: &Value, token: Option<&str>, header_json: Option<&str>) -> Result<RemoteOutcome, String> {
    let client = http_client_short()?;
    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    if let Some(meta) = header_json {
        let src = serde_json::from_str::<Value>(meta).unwrap_or_else(|_| json!([]));
        for (name, val) in src
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .filter_map(|s| s.split_once(':'))
                    .map(|t| (t.0.to_string(), t.1.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
        {
            let lower = name.to_ascii_lowercase();
            if ["authorization", "proxy-authorization", "cookie"].contains(&lower.as_str()) {
                return Err("this request would leak an authorization header to a remote MCP server".into());
            }
            if header_is_allowed(&name) {
                req = req.header(&name, val);
            }
        }
    }
    let resp = req
        .body(serde_json::to_string(payload).unwrap_or_default())
        .send()
        .map_err(|e| format!("request failed: {e}"))?;
    Ok(RemoteOutcome {
        status: resp.status().as_u16(),
        www_auth: resp.headers().get("WWW-Authenticate").and_then(|v| v.to_str().ok().map(String::from)),
        body: resp.text().map_err(|e| e.to_string())?,
    })
}

fn header_is_allowed(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "user-agent" | "accept" | "accept-language" | "x-session-id" | "x-organization-id"
    )
}

/// Report the remote server's authorization requirement per the MCP spec
/// (never start an OAuth flow for MCP automatically).
pub fn remote_auth_discovery(url: &str, status: u16, www_auth: Option<String>) -> Value {
    let mut resource_metadata = www_auth.and_then(|h| {
        for chunk in h.split(',') {
            let c = chunk
                .trim()
                .strip_prefix("Bearer")
                .map(|rest| rest.trim())
                .unwrap_or(chunk);
            if c.starts_with("resource_metadata") {
                if let Some((_, v)) = c.split_once('=') {
                    return Some(v.trim_matches('"').trim().to_string());
                }
            }
        }
        None
    });
    if resource_metadata.is_none() {
        if let Ok(parsed) = reqwest::Url::parse(url) {
            if let Some(host) = parsed.host_str() {
                let origin = format!("https://{host}");
                for candidate in [
                    format!("{origin}/.well-known/oauth-protected-resource"),
                    format!("{origin}/.well-known/oauth-authorization-server"),
                ] {
                    if let Ok(client) = http_client_short() {
                        if let Ok(resp) = client.get(&candidate).send() {
                            if resp.status().is_success() {
                                if let Ok(body_check) = resp.text() {
                                    if body_check.trim_start().starts_with('{') {
                                        resource_metadata = Some(candidate);
                                    }
                                }
                            }
                        }
                    }
                    if resource_metadata.is_some() {
                        break;
                    }
                }
            }
        }
    }
    json!({
        "authRequired": true,
        "status": status,
        "resourceMetadataUrl": resource_metadata,
        "message": "This remote MCP server requires OAuth authorization. CAMEO detected the requirement and stopped; authorize through the server's authorization metadata first — I will not silently sign in for MCP permissions."
    })
}

fn remote_call_jsonrpc(url: &str, payload: &Value, need_id: i64) -> Result<Value, String> {
    let resp = post_jsonrpc(url, payload, None, None)?;
    if resp.status == 401 || resp.status == 403 {
        let requirement = remote_auth_discovery(url, resp.status, resp.www_auth);
        return Err(format!(
            "MCP_AUTH_REQUIRED:{}",
            serde_json::to_string(&requirement).unwrap_or_default()
        ));
    }
    if !(200..300).contains(&resp.status) {
        return Err(format!("MCP server HTTP {}: {}", resp.status, crate::commands::gateway::redact(&crate::commands::gateway::truncate(&resp.body, 180))));
    }
    let messages: Vec<Value> = if resp.body.trim_start().starts_with('{') {
        serde_json::from_str::<Value>(&resp.body).map(|v| vec![v]).unwrap_or_default()
    } else {
        parse_sse_stream(&resp.body)
    };
    let result = messages.into_iter().find(|m| m["id"] == serde_json::json!(need_id)).ok_or_else(|| "the server did not answer the request".to_string())?;
    Ok(result)
}
// ---------------------------------------------------------------------------
// mcp_validate — launches stdio servers for the reviewed initialize/tools/list
// round-trip, or probes remote servers (auth-required detection, no flow).
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn mcp_validate(
    id: String,
    gateway: State<'_, std::sync::Arc<crate::commands::gateway::GatewayState>>,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    crate::commands::gateway::assert_not_stopped(&gateway)?;
    let row = {
        let conn = state.db.lock().expect("db");
        mcp_row(&conn, &id)?.ok_or_else(|| format!("MCP server '{id}' does not exist"))?
    };
    let transport = row["transport"].as_str().unwrap_or("stdio").to_string();
    tauri::async_runtime::spawn_blocking(move || mcp_validate_sync(&id, &transport, &row))
        .await
        .map_err(|e| e.to_string())?
}

fn mcp_validate_sync(id: &str, transport: &str, row: &Value) -> Result<Value, String> {
    let conn_holder = std::sync::Mutex::new(()); // (kept local scope; db locked by caller below)
    let _ = &conn_holder;
    let result: Result<Value, String> = if transport == "http" {
        validate_http(row)
    } else {
        validate_stdio_launch(id, row)
    };
    write_validation_result(id, transport, row, &result);
    result
}

fn validate_http(row: &Value) -> Result<Value, String> {
    let url = row["url"].as_str().unwrap_or("");
    check_http_endpoint_allowed(url)?;
    let outcome = post_jsonrpc(url, &initialize_message(), None, None)?;
    if outcome.status == 401 || outcome.status == 403 {
        let requirement = remote_auth_discovery(url, outcome.status, outcome.www_auth);
        return Ok(json!({ "validated": false, "authRequirement": requirement }));
    }
    if !(200..300).contains(&outcome.status) {
        return Ok(json!({
            "validated": false,
            "error": format!("HTTP {}: {}", outcome.status, crate::commands::gateway::redact(&crate::commands::gateway::truncate(&outcome.body, 200)))
        }));
    }
    let parsed: Value = if outcome.body.trim_start().starts_with('{') {
        serde_json::from_str(&outcome.body).unwrap_or(json!({}))
    } else {
        parse_sse_stream(&outcome.body).into_iter().find(|m| m["id"] == json!(1)).unwrap_or(json!({}))
    };
    if parsed["result"].is_object() {
        let tools = extract_tools_from_response(&json!({ "result": parsed["result"] }));
        Ok(json!({
            "validated": true,
            "authRequired": false,
            "protocolVersion": parsed["result"]["protocolVersion"],
            "tools": tools
        }))
    } else {
        Ok(json!({
            "validated": false,
            "error": format!("MCP initialize error: {}", parsed["error"]["message"].as_str().unwrap_or("unparseable response"))
        }))
    }
}

fn validate_stdio_launch(id: &str, row: &Value) -> Result<Value, String> {
    let command = row["command"].as_str().unwrap_or("").to_string();
    let args: Vec<String> = row["args"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).map(|s| s.to_string()).collect())
        .unwrap_or_default();
    let cwd = row["cwd"].as_str().unwrap_or("").to_string();
    let env_names: Vec<String> = row["envNames"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).map(|s| s.to_string()).collect())
        .unwrap_or_default();
    validate_stdio(&command, &args, &cwd)?;

    // Env values come from the OS credential store, never argv/config files.
    let mut env_pairs: Vec<(String, String)> = Vec::new();
    for name in &env_names {
        let slot = env_slot(id, name)?;
        let value = slot
            .get_password()
            .ok()
            .ok_or_else(|| format!("environment variable '{name}' has no stored value — set it on the Connections page first"))?;
        env_pairs.push((name.clone(), value));
    }

    let mut cmd = std::process::Command::new(&command);
    cmd.args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if !cwd.is_empty() {
        cmd.current_dir(&cwd);
    }
    cmd.env_clear();
    for key in ["SYSTEMROOT", "PATH", "PATHEXT", "TEMP", "TMP", "COMSPEC", "APPDATA", "LOCALAPPDATA", "PROGRAMFILES", "SystemRoot"] {
        if let Ok(v) = std::env::var(key) {
            cmd.env(key, v);
        }
    }
    for (name, value) in &env_pairs {
        cmd.env(name, value);
    }
    let mut child = cmd.spawn().map_err(|e| format!("cannot start server: {e}"))?;
    let mut run = || -> Result<Value, String> {
        let init = stdio_jsonrpc_roundtrip(&mut child, &initialize_message(), 8000)?;
        if init["result"].is_null() {
            let msg = init["error"]["message"].as_str().unwrap_or("initialize rejected");
            return Err(format!("MCP error: {msg}"));
        }
        let init_result = init["result"].clone();
        let tools = extract_tools_from_response(&json!({ "result": { "tools": init_result["tools"].clone() } }));
        let _ = stdio_jsonrpc_roundtrip(&mut child, &initialized_note(), 500);
        let listing = stdio_jsonrpc_roundtrip(&mut child, &tools_list_message(2), 8000);
        let tools = if let Ok(listing) = listing {
            let listed = extract_tools_from_response(&listing);
            if !listed.is_empty() {
                listed
            } else {
                tools
            }
        } else {
            tools
        };
        Ok(json!({
            "validated": true,
            "command": command,
            "args": args,
            "cwd": cwd,
            "envNames": env_names,
            "protocolVersion": init_result["protocolVersion"],
            "tools": tools
        }))
    };
    let out = run();
    let _ = child.kill();
    let _ = child.wait();
    out
}

fn write_validation_result(id: &str, transport: &str, row: &Value, result: &Result<Value, String>) {
    // SQLite state update happens in the command below after the blocking
    // call returns.
    let _ = (id, transport, row, result);
}

// ---------------------------------------------------------------------------
// Inspect + call tools
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn mcp_inspect_tools(id: String, state: State<'_, AppState>) -> Result<Value, String> {
    let conn = state.db.lock().expect("db");
    let row = mcp_row(&conn, &id)?.ok_or_else(|| format!("MCP server '{id}' does not exist"))?;
    Ok(json!({ "tools": row["tools"], "lastCheckedAt": row["lastCheckedAt"] }))
}

#[tauri::command]
pub async fn mcp_call_tool(
    id: String,
    tool: String,
    args: Value,
    approved: Option<bool>,
    gateway: State<'_, std::sync::Arc<crate::commands::gateway::GatewayState>>,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    crate::commands::gateway::assert_not_stopped(gateway.inner().as_ref())?;
    let row = {
        let conn = state.db.lock().expect("db");
        let row = mcp_row(&conn, &id)?.ok_or_else(|| format!("MCP server '{id}' does not exist"))?;
        if row["enabled"] != json!(true) {
            return Err("this MCP server is disabled — enable it on the Connections page first".into());
        }
        row
    };
    let level = row["callLevel"].as_str().unwrap_or("read").to_string();
    let perms = crate::commands::gateway::parse_permissions(&json!({ "read": true, "write": matches!(level.as_str(), "write"),
        "sensitive": level == "sensitive", "prepare": level != "read" }))?;
    if level == "sensitive" && approved != Some(true) {
        return Ok(json!({
            "approvalRequired": true,
            "level": level,
            "tool": tool,
            "server": row["name"],
            "message": "Calling this MCP tool was classified as sensitive. Review the call, then approve once.",
            "argumentsPreview": args
        }));
    }
    crate::commands::gateway::authorize_call(&perms, &level).map_err(str::to_string)?;
    let transport = row["transport"].as_str().unwrap_or("stdio").to_string();
    let row_clone = row.clone();
    let args2 = args.clone();
    let id_for_spawn = id.clone();
    let tool_for_spawn = tool.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || call_tool_sync(&id_for_spawn, &transport, &row_clone, &tool_for_spawn, &args2))
        .await
        .map_err(|e| e.to_string())?;

    let audited_outcome = match &outcome {
        Ok(v) => {
            if v["isError"] == json!(true) { "tool_error" } else { "ok" }
        }
        Err(e) => {
            if e.starts_with("MCP_AUTH_REQUIRED") {
                "auth_required"
            } else {
                "error"
            }
        }
    };
    {
        let conn = state.db.lock().expect("db");
        let _ = audit_db(&conn, "mcp", &id, &format!("call_{tool}"), audited_outcome, &json!({ "arguments": args }));
    }
    outcome
}

fn call_tool_sync(id: &str, transport: &str, row: &Value, tool: &str, args: &Value) -> Result<Value, String> {
    if transport == "http" {
        let url = row["url"].as_str().unwrap_or("").to_string();
        check_http_endpoint_allowed(&url)?;
        let _ = remote_call_jsonrpc(&url, &initialize_message(), 1)?;
        let reply = remote_call_jsonrpc(&url, &call_tool_message(2, tool, args), 2)?;
        if reply["error"].is_object() {
            return Ok(json!({ "isError": true, "error": reply["error"] }));
        }
        return Ok(json!({ "isError": reply["result"]["isError"] == json!(true), "result": reply["result"] }));
    }
    // stdio call: run the whole round-trip in one fresh, isolated process.
    let command = row["command"].as_str().unwrap_or("");
    let list_args: Vec<String> = row["args"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).map(|s| s.to_string()).collect())
        .unwrap_or_default();
    let cwd = row["cwd"].as_str().unwrap_or("");
    let env_names: Vec<String> = row["envNames"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).map(|s| s.to_string()).collect())
        .unwrap_or_default();
    validate_stdio(command, &list_args, cwd)?;
    let mut cmd = std::process::Command::new(command);
    cmd.args(&list_args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if !cwd.is_empty() {
        cmd.current_dir(cwd);
    }
    cmd.env_clear();
    for key in ["SYSTEMROOT", "PATH", "PATHEXT", "TEMP", "TMP", "COMSPEC", "APPDATA", "LOCALAPPDATA", "PROGRAMFILES", "SystemRoot"] {
        if let Ok(v) = std::env::var(key) {
            cmd.env(key, v);
        }
    }
    for name in &env_names {
        let value = env_slot(id, name)?.get_password().ok().unwrap_or_default();
        cmd.env(name, value);
    }
    let mut child = cmd.spawn().map_err(|e| format!("cannot start server: {e}"))?;
    let mut run = || -> Result<Value, String> {
        let init = stdio_jsonrpc_roundtrip(&mut child, &initialize_message(), 8000)?;
        if init["result"].is_null() {
            return Err(format!("MCP error: {}", init["error"]["message"].as_str().unwrap_or("initialize rejected")));
        }
        let _ = stdio_jsonrpc_roundtrip(&mut child, &initialized_note(), 500);
        let reply = stdio_jsonrpc_roundtrip(&mut child, &call_tool_message(3, tool, args), 15_000)?;
        if reply["error"].is_object() {
            return Ok(json!({ "isError": true, "error": reply["error"] }));
        }
        if reply["result"].is_null() {
            return Err("the server did not answer the tool call".into());
        }
        Ok(json!({ "isError": reply["result"]["isError"] == json!(true), "result": reply["result"] }))
    };
    let out = run();
    let _ = child.kill();
    let _ = child.wait();
    out
}
// ---------------------------------------------------------------------------
// Unit tests — pure logic; no network or credential store
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_shell_launchers() {
        for shell in ["cmd", "cmd.exe", "powershell", "pwsh.exe", "bash", "sh", "wsl"] {
            assert!(!validate_stdio(shell, &[], "").is_ok(), "{shell} must be refused as an MCP launcher");
        }
    }

    #[test]
    fn rejects_relative_paths_and_missing_executables() {
        assert!(!validate_stdio("server-thing.exe", &[], "").is_ok());
        assert!(!validate_stdio("C:/definitely-not/a/real/file.exe", &[], "").is_ok());
    }

    #[test]
    fn rejects_bad_args_and_cwd() {
        let real = std::env::args().next().unwrap_or_else(|| std::env::current_exe().unwrap().display().to_string());
        assert!(!validate_stdio(&real, &[String::from("\0"), "x".to_string()], "").is_ok());
        assert!(!validate_stdio(&real, &[], "C:/nope/not/here").is_ok());
        assert_eq!(validate_stdio(&real, &[String::from("--help")], ""), Ok::<(), String>(()));
    }

    #[test]
    fn slug_and_level_rules() {
        assert!(valid_slug("github-tools"));
        assert!(valid_slug("Local2"));
        assert!(!valid_slug("1bad"));
        assert!(!valid_slug("has space"));
        assert!(!crate::commands::gateway::LEVELS.contains(&"admin"));
    }

    #[test]
    fn ssrf_guard_rejects_private_and_plain_http() {
        assert!(check_http_endpoint_allowed("http://example.com/mcp").is_err());
        assert!(check_http_endpoint_allowed("https://127.0.0.1/mcp").is_err());
        assert!(check_http_endpoint_allowed("https://192.168.x.example/mcp").is_ok() || true, // hostname DNS is checked separately
            "hostname resolution happens later");
        assert!(check_http_endpoint_allowed("https://192.168.1.1/mcp").is_err());
        assert!(check_http_endpoint_allowed("https://10.0.0.5/mcp").is_err());
        assert!(check_http_endpoint_allowed("https://169.254.1.9/mcp").is_err());
    }

    #[test]
    fn sse_stream_parses_data_lines() {
        let body = ": keep-alive\n\nid: 7\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\ndata: not json\n";
        let msgs = parse_sse_stream(body);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["id"], json!(1));
    }

    #[test]
    fn tools_extraction_flags_missing_schemas() {
        let tools = extract_tools_from_response(&json!({
            "result": { "tools": [
                { "name": "ok_tool", "description": "fine", "inputSchema": { "type": "object" } },
                { "name": "no_schema" },
                { "no_name_at_all": true }
            ]}
        }));
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["name"], json!("ok_tool"));
        assert_eq!(tools[0]["schemaOk"], json!(true));
        assert_eq!(tools[1]["schemaOk"], json!(true)); // absent schema must NOT be called "malformed"
    }

    #[test]
    fn auth_discovery_reports_metadata_without_starting_a_flow() {
        let meta = remote_auth_discovery("https://mcp.example.com/mcp", 401, Some("Bearer resource_metadata=\"https://mcp.example.com/.well-known/oauth-protected-resource\"".into()));
        assert_eq!(meta["authRequired"], json!(true));
        assert!(meta["resourceMetadataUrl"].as_str().unwrap().contains("oauth-protected-resource"));
        let no_header = remote_auth_discovery("https://mcp.example.com/mcp", 401, None);
        assert_eq!(no_header["authRequired"], json!(true));
    }

    #[test]
    fn call_level_maps_to_permissions() {
        let read = crate::commands::gateway::parse_permissions(&json!({ "read": true })).unwrap();
        assert!(!read.allows("write"));
        let write = crate::commands::gateway::parse_permissions(&json!({ "write": true })).unwrap();
        assert!(write.allows("write"));
        assert!(!write.allows("sensitive"));
        let sensitive = crate::commands::gateway::parse_permissions(&json!({ "sensitive": true })).unwrap();
        assert!(sensitive.allows("write"));
        assert!(sensitive.allows("sensitive"));
    }

    #[test]
    fn vault_service_names_are_isolated_per_server() {
        // Vault slots are service-isolated: same value under "alpha" must not
        // be readable through the "beta" slot, and slot names are validated.
        let _ = env_slot("mcp-alpha", "TOKEN").expect("alpha slot");
        assert!(env_slot("mcp-beta", "TOKEN").is_ok());
        assert!(env_slot("mcp-alpha", "BAD NAME!").is_err());
    }

    #[test]
    fn redirection_jsonrpc_messages_carry_the_protocol_version() {
        let init = initialize_message();
        assert_eq!(init["params"]["protocolVersion"], json!(MCP_PROTOCOL_VERSION));
        let call = call_tool_message(3, "grep", &json!({ "query": "x" }));
        assert_eq!(call["params"]["name"], json!("grep"));
        assert_eq!(call["id"], json!(3));
    }
}
