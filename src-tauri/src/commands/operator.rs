//! Operator capabilities: scoped command execution, live web fetch and device launch.
//!
//! Autonomy is opt-in (`autonomy_enabled`) and every capability is gated by its
//! own toggle. Command execution is confined to one project root the user picks,
//! and destructive/irreversible commands require an explicit `approved` flag.
//! Every attempt — allowed or refused — is written to the audit log.

use crate::commands::gateway::{audit_db, now_ms};
use crate::commands::store::AppState;
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tauri::State;

const PREF_ENABLED: &str = "autonomy_enabled";
const PREF_ROOT: &str = "exec_root";
const PREF_NET: &str = "autonomy_net";
const PREF_DEVICE: &str = "autonomy_device";
/// Explicit owner opt-in to lift the project-root confinement: agents may
/// then use any folder (including drive roots) as their working scope.
/// Destructive-command confirmation and audit logging stay on regardless.
const PREF_FULL: &str = "operator_full_access";

fn full_access_enabled(state: &AppState) -> bool {
    is_on(state, PREF_FULL)
}

const EXEC_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_OUTPUT_CHARS: usize = 24_000;
const MAX_FETCH_BYTES: usize = 2 * 1024 * 1024;
const SEARCH_PROVIDER: &str = "exa";
const SEARCH_ENDPOINT: &str = "https://api.exa.ai/search";
const MAX_SEARCH_RESULTS: u8 = 10;
const MAX_QUERY_CHARS: usize = 500;
const MAX_SEARCH_HIGHLIGHT_CHARS: usize = 1_000;
const MAX_SEARCH_HIGHLIGHTS_PER_RESULT: usize = 3;

fn prefs_get(state: &AppState, key: &str) -> Option<String> {
    let db = state.db.lock().ok()?;
    db.query_row("SELECT value FROM prefs WHERE key = ?1", [key], |r| r.get::<_, String>(0))
        .ok()
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

fn audit(state: &AppState, action: &str, outcome: &str, detail: Value) {
    if let Ok(db) = state.db.lock() {
        let _ = audit_db(&db, "operator", "local", action, outcome, &detail);
    }
}

fn is_on(state: &AppState, key: &str) -> bool {
    prefs_get(state, key).as_deref() == Some("1")
}

fn search_keyring_entry() -> Result<keyring::Entry, String> {
    // Exa web-search credentials live beside—but namespaced apart from—AI provider keys.
    keyring::Entry::new("cameo_search_exa", "api_key").map_err(|e| format!("credential store error: {e}"))
}

fn get_search_key_sync() -> Result<Option<String>, String> {
    match search_keyring_entry() {
        Ok(entry) => match entry.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("credential store error: {e}")),
        },
        Err(e) => Err(e),
    }
}

fn validate_search_api_key(key: &str) -> Result<String, String> {
    let trimmed = key.trim().to_string();
    if trimmed.chars().count() < 8 || trimmed.chars().count() > 256 {
        return Err("that search API key looks incomplete".into());
    }
    Ok(trimmed)
}

#[tauri::command]
pub async fn search_set_key(key: String) -> Result<bool, String> {
    let key = validate_search_api_key(&key)?;
    tauri::async_runtime::spawn_blocking(move || {
        let entry = search_keyring_entry()?;
        entry
            .set_password(&key)
            .map(|_| true)
            .map_err(|e| format!("failed to save search key: {e}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn search_delete_key() -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let entry = search_keyring_entry()?;
        match entry.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(format!("failed to delete search key: {e}")),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn search_has_key() -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || Ok(get_search_key_sync()?.is_some()))
        .await
        .map_err(|e| e.to_string())?
}

fn autonomy_enabled(state: &AppState) -> bool {
    is_on(state, PREF_ENABLED)
}

fn autonomy_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::limited(3))
        .user_agent("CAMEO/0.1 (+https://cameo.local)")
        .build()
        .map_err(|e| format!("http client error: {e}"))
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ValidExecArgs {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
}

pub(crate) fn validate_exec_args(args: &Value) -> Result<ValidExecArgs, String> {
    let command = args
        .get("command")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("exec needs a program name")?
        .to_string();
    if command.chars().any(|c| c.is_whitespace() || matches!(c, ';' | '|' | '&' | '>' | '<')) {
        return Err("pass the program name alone and put its arguments in `args`".into());
    }
    let mut validated = Vec::new();
    if let Some(items) = args.get("args").and_then(Value::as_array) {
        if items.len() > 20 {
            return Err("exec accepts at most 20 arguments".into());
        }
        for item in items {
            let text = item.as_str().ok_or("exec arguments must be strings")?;
            if text.chars().count() > 1_000 {
                return Err("one exec argument is too long".into());
            }
            validated.push(text.to_string());
        }
    }
    let cwd = args
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.chars().count() > 500 {
                Err("exec working directory is too long")
            } else {
                Ok(s.to_string())
            }
        })
        .transpose()?;
    Ok(ValidExecArgs { command, args: validated, cwd })
}

pub(crate) fn validate_fetch_args(args: &Value) -> Result<String, String> {
    let url = args
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("fetch needs a URL")?;
    if url.chars().count() > 2_048 {
        return Err("fetch URL is too long".into());
    }
    Ok(url.to_string())
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ValidSearchArgs {
    pub query: String,
    pub count: u8,
}

pub(crate) fn validate_search_args(args: &Value) -> Result<ValidSearchArgs, String> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("search needs a query")?;
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(format!("search query is limited to {MAX_QUERY_CHARS} characters"));
    }
    let count = args
        .get("num_results")
        .and_then(Value::as_u64)
        .unwrap_or(5)
        .clamp(1, u64::from(MAX_SEARCH_RESULTS)) as u8;
    Ok(ValidSearchArgs { query: query.to_string(), count })
}

pub(crate) fn validate_device_args(args: &Value) -> Result<String, String> {
    let target = args
        .get("target")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("device launch needs a target")?;
    if target.chars().count() > 1_000 {
        return Err("device target is too long".into());
    }
    Ok(target.to_string())
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SearchHit {
    pub url: String,
    pub title: String,
    pub published_date: Option<String>,
    pub highlights: Vec<String>,
}

pub(crate) fn search_request_body(query: &str, count: u8) -> Value {
    json!({
        "query": query,
        "type": "auto",
        "numResults": count,
        "contents": { "highlights": true },
    })
}

fn truncate_highlight(text: &str) -> String {
    truncate_chars(text.to_string(), MAX_SEARCH_HIGHLIGHT_CHARS).0
}

pub(crate) fn parse_search_response(body: &Value) -> Result<Vec<SearchHit>, String> {
    let results = body
        .get("results")
        .and_then(Value::as_array)
        .ok_or("search provider returned an unexpected response")?;
    let mut hits = Vec::new();
    for item in results.iter().take(usize::from(MAX_SEARCH_RESULTS)) {
        let url = item
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or("search provider returned a result without a URL")?;
        let title = item
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let published_date = item
            .get("publishedDate")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let mut highlights = Vec::new();
        if let Some(items) = item.get("highlights").and_then(Value::as_array) {
            // Filter blanks first so empty strings cannot eat the bounded slots.
            for highlight in items
                .iter()
                .filter_map(|h| h.as_str().map(str::trim).filter(|s| !s.is_empty()))
                .take(MAX_SEARCH_HIGHLIGHTS_PER_RESULT)
            {
                highlights.push(truncate_highlight(highlight));
            }
        }
        hits.push(SearchHit { url: url.to_string(), title, published_date, highlights });
    }
    Ok(hits)
}

fn require_autonomy(state: &AppState, capability: &str, key: &str) -> Result<(), String> {
    if !autonomy_enabled(state) {
        return Err("Autonomy is off. Enable it in Settings → Autonomy first.".into());
    }
    if !is_on(state, key) {
        return Err(format!("{capability} is turned off in Settings → Autonomy."));
    }
    Ok(())
}

/// The canonical project root commands are confined to.
fn exec_root(state: &AppState) -> Result<PathBuf, String> {
    let raw = prefs_get(state, PREF_ROOT)
        .filter(|r| !r.trim().is_empty())
        .ok_or_else(|| "No project root selected. Pick one in Settings → Autonomy.".to_string())?;
    let root = std::fs::canonicalize(raw.trim())
        .map_err(|e| format!("project root '{}' is not accessible: {e}", raw.trim()))?;
    if !root.is_dir() {
        return Err("the selected project root is not a directory".into());
    }
    Ok(root)
}

/// Resolve a caller-supplied working directory inside `root`. Absolute paths and
/// `..` traversal are only accepted when the canonical result stays under `root` —
/// unless the owner enabled full access, in which case any accessible absolute
/// path is allowed (destructive-command confirmation still applies per call).
fn resolve_within(root: &Path, relative: &str, full_access: bool) -> Result<PathBuf, String> {
    let relative = relative.trim();
    let candidate = if relative.is_empty() {
        root.to_path_buf()
    } else {
        let p = Path::new(relative);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            root.join(p)
        }
    };
    let resolved = std::fs::canonicalize(&candidate)
        .map_err(|e| format!("path '{}' is not accessible: {e}", candidate.display()))?;
    if !full_access && !resolved.starts_with(root) {
        return Err(format!("path '{}' is outside the project root", resolved.display()));
    }
    Ok(resolved)
}

/// Why a command needs the user's explicit confirmation, if it does.
/// Pure so the guard rails are unit-tested rather than trusted.
pub(crate) fn destructive_reason(command: &str, args: &[String]) -> Option<&'static str> {
    let base = command
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(command)
        .to_ascii_lowercase();
    let base = base.trim_end_matches(".exe").trim_end_matches(".cmd").trim_end_matches(".bat");
    let argv: Vec<String> = args.iter().map(|a| a.to_ascii_lowercase()).collect();
    let has = |needle: &str| argv.iter().any(|a| a == needle || a.starts_with(&format!("{needle}=")));

    match base {
        "rm" | "rimraf" => {
            let recursive = argv.iter().any(|a| a.starts_with('-') && a.contains('r'));
            let force = argv.iter().any(|a| a.starts_with('-') && a.contains('f'));
            if recursive || force {
                return Some("recursive/forced delete");
            }
        }
        "rmdir" | "rd" | "del" | "erase" | "remove-item" | "clear-disk" | "format" | "diskpart" => {
            return Some("file/system deletion")
        }
        "shutdown" | "restart-computer" | "stop-computer" | "bcdedit" | "taskkill" | "stop-process" => {
            return Some("shuts down or alters running processes")
        }
        "reg" => {
            if has("delete") || has("add") {
                return Some("registry modification");
            }
        }
        "git" => {
            if has("push") && (has("--force") || has("-f") || has("--force-with-lease")) {
                return Some("force-push rewrites remote history");
            }
            if has("reset") && has("--hard") {
                return Some("hard reset discards uncommitted work");
            }
            if has("clean") && argv.iter().any(|a| a.starts_with("-f")) {
                return Some("git clean deletes untracked files");
            }
        }
        "npm" | "yarn" | "pnpm" | "bun" => {
            if has("publish") {
                return Some("publishes a package publicly");
            }
            if has("uninstall") || has("remove") {
                return Some("removes installed packages");
            }
        }
        "cargo" => {
            if has("publish") {
                return Some("publishes a crate publicly");
            }
            if has("uninstall") {
                return Some("removes installed binaries");
            }
        }
        "pip" | "pip3" => {
            if has("uninstall") {
                return Some("removes installed packages");
            }
        }
        _ => {}
    }
    None
}

/// Quote one argument for `cmd.exe`. Anything containing shell metacharacters is
/// wrapped and inner quotes are doubled.
fn quote_win(arg: &str) -> String {
    let needs = arg.is_empty()
        || arg
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '&' | '|' | '<' | '>' | '^' | '(' | ')'));
    if needs {
        format!("\"{}\"", arg.replace('"', "\"\""))
    } else {
        arg.to_string()
    }
}

#[cfg(windows)]
fn build_command(command: &str, args: &[String]) -> Command {
    // `npm`, `cargo fmt` shims etc. are .cmd files on Windows, so go through the
    // command processor instead of relying on PATHEXT resolution.
    let mut line = quote_win(command);
    for a in args {
        line.push(' ');
        line.push_str(&quote_win(a));
    }
    let mut cmd = Command::new("cmd");
    cmd.arg("/C").arg(line);
    cmd
}

#[cfg(not(windows))]
fn build_command(command: &str, args: &[String]) -> Command {
    let mut cmd = Command::new(command);
    cmd.args(args);
    cmd
}

fn truncate_chars(input: String, max: usize) -> (String, bool) {
    if input.chars().count() <= max {
        return (input, false);
    }
    (input.chars().take(max).collect(), true)
}

struct ExecResult {
    exit_code: i32,
    stdout: String,
    stderr: String,
    truncated: bool,
    timed_out: bool,
    duration_ms: u64,
}

fn run_command(mut cmd: Command, timeout: Duration) -> Result<ExecResult, String> {
    let started = Instant::now();
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to start command: {e}"))?;

    let out_pipe = child.stdout.take().ok_or("failed to capture stdout")?;
    let err_pipe = child.stderr.take().ok_or("failed to capture stderr")?;
    let out_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out_pipe.take((MAX_OUTPUT_CHARS * 4) as u64).read_to_string(&mut s);
        s
    });
    let err_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err_pipe.take((MAX_OUTPUT_CHARS * 4) as u64).read_to_string(&mut s);
        s
    });

    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if started.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            Err(e) => return Err(format!("failed while waiting for command: {e}")),
        }
    };

    let out_raw = out_reader.join().unwrap_or_default();
    let err_raw = err_reader.join().unwrap_or_default();
    let (stdout, out_cut) = truncate_chars(out_raw, MAX_OUTPUT_CHARS);
    let (stderr, err_cut) = truncate_chars(err_raw, MAX_OUTPUT_CHARS);
    Ok(ExecResult {
        exit_code: status.and_then(|s| s.code()).unwrap_or(-1),
        stdout,
        stderr,
        truncated: out_cut || err_cut,
        timed_out,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

fn status_json(state: &AppState) -> Value {
    json!({
        "enabled": autonomy_enabled(state),
        "root": prefs_get(state, PREF_ROOT).filter(|r| !r.trim().is_empty()),
        "net": is_on(state, PREF_NET),
        "device": is_on(state, PREF_DEVICE),
        "fullAccess": full_access_enabled(state),
    })
}

/// Explicit owner opt-in to full system scope. Recorded in prefs and the
/// audit log; destructive-command confirmation still applies to every call.
#[tauri::command]
pub fn operator_set_full_access(full: bool, state: State<'_, AppState>) -> Result<Value, String> {
    prefs_set(&state, PREF_FULL, if full { "1" } else { "0" });
    audit(
        &state,
        "full_access",
        if full { "enabled" } else { "disabled" },
        json!({ "at": now_ms() }),
    );
    Ok(status_json(&state))
}

#[tauri::command]
pub fn operator_status(state: State<'_, AppState>) -> Result<Value, String> {
    Ok(status_json(&state))
}

/// Turn autonomy on/off and grant/revoke the individual capabilities.
#[tauri::command]
pub fn operator_configure(
    enabled: bool,
    net: bool,
    device: bool,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    prefs_set(&state, PREF_ENABLED, if enabled { "1" } else { "0" });
    prefs_set(&state, PREF_NET, if net { "1" } else { "0" });
    prefs_set(&state, PREF_DEVICE, if device { "1" } else { "0" });
    audit(
        &state,
        "configure",
        "ok",
        json!({ "enabled": enabled, "net": net, "device": device, "at": now_ms() }),
    );
    Ok(status_json(&state))
}

/// Pin the single directory agents may execute in.
#[tauri::command]
pub fn operator_set_root(path: String, state: State<'_, AppState>) -> Result<Value, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("choose a project root first".into());
    }
    let root = std::fs::canonicalize(trimmed)
        .map_err(|e| format!("'{trimmed}' is not accessible: {e}"))?;
    if !root.is_dir() {
        return Err("the project root must be a directory".into());
    }
    // Drive roots are only accepted when the owner explicitly enabled full
    // access; otherwise a whole volume is not a scope.
    if !full_access_enabled(&state) {
        let components: Vec<Component> = root.components().collect();
        if components.len() <= 1 {
            return Err("pick a project folder, not a drive root".into());
        }
    }
    prefs_set(&state, PREF_ROOT, &root.to_string_lossy());
    audit(&state, "set_root", "ok", json!({ "root": root.to_string_lossy() }));
    Ok(status_json(&state))
}

pub(crate) async fn exec_run_impl(
    state: &AppState,
    command: &str,
    args: &[String],
    cwd: Option<&str>,
    approved: bool,
) -> Result<Value, String> {
    require_autonomy(state, "Command execution", PREF_ENABLED).map_err(|e| {
        audit(state, "exec", "blocked", json!({ "reason": "autonomy_off" }));
        e
    })?;
    let command = command.trim().to_string();
    if command.is_empty() {
        return Err("no command given".into());
    }
    if command.chars().any(|c| c.is_whitespace() || matches!(c, ';' | '|' | '&' | '>' | '<')) {
        return Err("pass the program name alone and put its arguments in `args`".into());
    }
    let root = exec_root(state)?;
    let workdir = resolve_within(&root, cwd.as_deref().unwrap_or(""), full_access_enabled(state))?;

    if let Some(reason) = destructive_reason(&command, &args) {
        if !approved {
            audit(
                state,
                "exec",
                "needs_approval",
                json!({ "command": command, "args": args, "reason": reason, "cwd": workdir }),
            );
            return Err(format!(
                "NEEDS_CONFIRMATION: '{command}' looks like {reason}. Nothing ran. Repeat with approved=true once you have confirmed it."
            ));
        }
    }

    let audit_detail = json!({
        "command": command,
        "args": args,
        "cwd": workdir.to_string_lossy(),
        "approved": approved,
    });
    let command_for_run = command.clone();
    let args_for_run = args.to_vec();
    let workdir_for_run = workdir.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut cmd = build_command(&command_for_run, &args_for_run);
        cmd.current_dir(&workdir_for_run);
        run_command(cmd, EXEC_TIMEOUT)
    })
    .await
    .map_err(|e| e.to_string())?;

    match result {
        Ok(r) => {
            audit(
                state,
                "exec",
                if r.timed_out { "timeout" } else if r.exit_code == 0 { "ok" } else { "failed" },
                json!({
                    "command": audit_detail["command"],
                    "args": audit_detail["args"],
                    "cwd": audit_detail["cwd"],
                    "exitCode": r.exit_code,
                    "durationMs": r.duration_ms,
                }),
            );
            Ok(json!({
                "exitCode": r.exit_code,
                "stdout": r.stdout,
                "stderr": r.stderr,
                "truncated": r.truncated,
                "timedOut": r.timed_out,
                "durationMs": r.duration_ms,
                "cwd": audit_detail["cwd"],
            }))
        }
        Err(e) => {
            audit(state, "exec", "error", json!({ "command": command, "error": e }));
            Err(e)
        }
    }
}

#[tauri::command]
pub async fn exec_run(
    command: String,
    args: Vec<String>,
    cwd: Option<String>,
    approved: bool,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    let state = state.inner().clone();
    exec_run_impl(&state, &command, &args, cwd.as_deref(), approved).await
}

/// Parsed/validated fetch target. https only; private, loopback and link-local
/// addresses are rejected (the same boundary MCP servers use).
pub(crate) fn validate_fetch_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|e| format!("invalid URL: {e}"))?;
    if url.scheme() != "https" {
        return Err("only https:// URLs can be fetched".into());
    }
    let host = url.host_str().ok_or("URL has no host")?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err("refusing to fetch localhost".into());
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if !crate::commands::mcp::ip_is_public_ip(&ip) {
            return Err("refusing to fetch a private address".into());
        }
    }
    Ok(url)
}

pub(crate) fn host_resolves_public(host: &str) -> bool {
    use std::net::ToSocketAddrs;
    match (host, 443u16).to_socket_addrs() {
        Ok(addrs) => {
            let mut any = false;
            for addr in addrs {
                any = true;
                if !crate::commands::mcp::ip_is_public_ip(&addr.ip()) {
                    return false;
                }
            }
            any
        }
        Err(_) => false,
    }
}

pub(crate) async fn net_fetch_impl(state: &AppState, url: &str) -> Result<Value, String> {
    require_autonomy(state, "Internet access", PREF_NET).map_err(|e| {
        audit(state, "fetch", "blocked", json!({ "reason": "disabled" }));
        e
    })?;
    let raw = url.trim().to_string();
    if raw.is_empty() {
        return Err("nothing to fetch".into());
    }
    let parsed = validate_fetch_url(&raw)?;
    let host = parsed.host_str().unwrap_or_default().to_string();
    if host.parse::<std::net::IpAddr>().is_err() && !host_resolves_public(&host) {
        audit(state, "fetch", "blocked", json!({ "url": raw, "reason": "private_dns" }));
        return Err(format!("'{host}' does not resolve to a public address"));
    }
    let client = autonomy_http_client()?;
    let resp = client
        .get(parsed.clone())
        .send()
        .await
        .map_err(|e| format!("fetch failed: {e}"))?;
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if let Some(len) = resp.content_length() {
        if len as usize > MAX_FETCH_BYTES {
            audit(state, "fetch", "blocked", json!({ "url": raw, "reason": "too_large", "bytes": len }));
            return Err(format!("response is {len} bytes; the {MAX_FETCH_BYTES}-byte limit applies"));
        }
    }
    let bytes = resp.bytes().await.map_err(|e| format!("failed reading body: {e}"))?;
    let truncated = bytes.len() > MAX_FETCH_BYTES;
    let slice = &bytes[..bytes.len().min(MAX_FETCH_BYTES)];
    let body = String::from_utf8_lossy(slice).to_string();
    audit(
        state,
        "fetch",
        "ok",
        json!({ "url": parsed.as_str(), "status": status, "bytes": bytes.len() }),
    );
    Ok(json!({
        "status": status,
        "url": parsed.as_str(),
        "contentType": content_type,
        "truncated": truncated,
        "bytes": bytes.len(),
        "body": body,
    }))
}

#[tauri::command]
pub async fn net_fetch(url: String, state: State<'_, AppState>) -> Result<Value, String> {
    let state = state.inner().clone();
    net_fetch_impl(&state, &url).await
}

pub(crate) fn device_launch_impl(state: &AppState, target: &str) -> Result<Value, String> {
    require_autonomy(state, "Device control", PREF_DEVICE).map_err(|e| {
        audit(state, "device", "blocked", json!({ "reason": "disabled" }));
        e
    })?;
    let target = target.trim().to_string();
    if target.is_empty() {
        return Err("nothing to launch".into());
    }
    #[cfg(windows)]
    let spawn = Command::new("cmd").args(["/C", "start", "", &target]).spawn();
    #[cfg(target_os = "macos")]
    let spawn = Command::new("open").arg(&target).spawn();
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let spawn = Command::new("xdg-open").arg(&target).spawn();
    match spawn {
        Ok(_) => {
            audit(state, "device", "ok", json!({ "target": target }));
            Ok(json!({ "launched": true, "target": target }))
        }
        Err(e) => {
            audit(state, "device", "error", json!({ "target": target, "error": e.to_string() }));
            Err(format!("failed to launch '{target}': {e}"))
        }
    }
}

#[tauri::command]
pub fn device_launch(target: String, state: State<'_, AppState>) -> Result<Value, String> {
    let state = state.inner().clone();
    device_launch_impl(&state, &target)
}

pub(crate) fn audit_agent_action(state: &AppState, action: &str, outcome: &str, detail: Value) {
    audit(state, action, outcome, detail);
}

pub(crate) async fn net_search_impl(state: &AppState, query: &str, count: u8) -> Result<Value, String> {
    require_autonomy(state, "Internet access", PREF_NET).map_err(|e| {
        audit(state, "search", "blocked", json!({ "reason": "disabled" }));
        e
    })?;
    let query = query.trim().to_string();
    if query.is_empty() {
        return Err("nothing to search".into());
    }
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(format!("search query is limited to {MAX_QUERY_CHARS} characters"));
    }
    let count = count.clamp(1, MAX_SEARCH_RESULTS);
    let key = get_search_key_sync()?.ok_or_else(|| {
        audit(state, "search", "blocked", json!({ "reason": "missing_key" }));
        "No Exa search key is saved. Add one in Settings → Autonomy before searching.".to_string()
    })?;
    let client = autonomy_http_client()?;
    let resp = client
        .post(SEARCH_ENDPOINT)
        .bearer_auth(key)
        .json(&search_request_body(&query, count))
        .send()
        .await
        .map_err(|e| {
            audit(state, "search", "error", json!({ "query": query, "error": e.to_string() }));
            format!("search failed: {e}")
        })?;
    let status = resp.status().as_u16();
    if status == 401 || status == 403 {
        audit(state, "search", "blocked", json!({ "reason": "invalid_key", "status": status }));
        return Err("The saved Exa key was rejected. Replace it in Settings → Autonomy.".into());
    }
    if status == 429 {
        audit(state, "search", "blocked", json!({ "reason": "rate_limited", "status": status }));
        return Err("Exa rate-limited this search. Wait and try a narrower query.".into());
    }
    if status < 200 || status >= 300 {
        let text = resp.text().await.unwrap_or_default();
        audit(state, "search", "error", json!({ "query": query, "status": status }));
        return Err(format!("search failed with HTTP {status}: {}", truncate_chars(text, 300).0));
    }
    let body: Value = resp.json().await.map_err(|e| format!("search returned invalid JSON: {e}"))?;
    let hits = parse_search_response(&body)?;
    audit(
        state,
        "search",
        "ok",
        json!({ "provider": SEARCH_PROVIDER, "query": query, "requested": count, "returned": hits.len() }),
    );
    Ok(json!({
        "provider": SEARCH_PROVIDER,
        "query": query,
        "requested": count,
        "returned": hits.len(),
        "results": hits
            .into_iter()
            .map(|hit| json!({
                "url": hit.url,
                "title": hit.title,
                "publishedDate": hit.published_date,
                "highlights": hit.highlights,
            }))
            .collect::<Vec<_>>(),
    }))
}

#[tauri::command]
pub async fn net_search(query: String, num_results: Option<u8>, state: State<'_, AppState>) -> Result<Value, String> {
    let state = state.inner().clone();
    net_search_impl(&state, &query, num_results.unwrap_or(5)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn ordinary_commands_are_not_flagged() {
        assert!(destructive_reason("cargo", &argv(&["test"])).is_none());
        assert!(destructive_reason("git", &argv(&["status"])).is_none());
        assert!(destructive_reason("git", &argv(&["push", "origin", "main"])).is_none());
        assert!(destructive_reason("npm", &argv(&["run", "build"])).is_none());
        assert!(destructive_reason("python", &argv(&["-m", "pytest"])).is_none());
    }

    #[test]
    fn destructive_commands_are_flagged_with_a_reason() {
        assert_eq!(destructive_reason("rm", &argv(&["-rf", "dist"])), Some("recursive/forced delete"));
        assert_eq!(destructive_reason("cmd.exe", &argv(&["/c", "del", "x"])), None); // argv only, not the line
        assert_eq!(destructive_reason("git", &argv(&["push", "--force"])), Some("force-push rewrites remote history"));
        assert_eq!(destructive_reason("git", &argv(&["reset", "--hard"])), Some("hard reset discards uncommitted work"));
        assert_eq!(destructive_reason("npm", &argv(&["publish"])), Some("publishes a package publicly"));
        assert_eq!(destructive_reason("shutdown", &argv(&["/s"])), Some("shuts down or alters running processes"));
        assert_eq!(destructive_reason("C:\\Windows\\System32\\reg.exe", &argv(&["delete", "HKCU\\x"])), Some("registry modification"));
    }

    #[test]
    fn resolve_within_rejects_traversal_and_absolute_escapes() {
        let root = std::fs::canonicalize(std::env::temp_dir()).expect("temp dir");
        let base = root.join("cameo_scope_test");
        let nested = base.join("nested");
        std::fs::create_dir_all(&nested).expect("mkdir");
        let sibling = root.join("cameo_scope_test_outside");
        std::fs::create_dir_all(&sibling).expect("mkdir");

        assert_eq!(resolve_within(&base, "", false).unwrap(), std::fs::canonicalize(&base).unwrap());
        assert_eq!(resolve_within(&base, "nested", false).unwrap(), std::fs::canonicalize(&nested).unwrap());
        assert!(resolve_within(&base, "../cameo_scope_test_outside", false).is_err());
        assert!(resolve_within(&base, sibling.to_string_lossy().as_ref(), false).is_err());
        // Full access (explicit owner opt-in) lifts the confinement.
        assert_eq!(
            resolve_within(&base, sibling.to_string_lossy().as_ref(), true).unwrap(),
            std::fs::canonicalize(&sibling).unwrap()
        );

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&sibling);
    }

    #[test]
    fn fetch_url_only_accepts_public_https() {
        assert!(validate_fetch_url("https://example.com/data.json").is_ok());
        assert!(validate_fetch_url("http://example.com").is_err());
        assert!(validate_fetch_url("file:///C:/Windows/win.ini").is_err());
        assert!(validate_fetch_url("https://localhost:8080/admin").is_err());
        assert!(validate_fetch_url("https://127.0.0.1/").is_err());
        assert!(validate_fetch_url("https://192.168.1.1/").is_err());
        assert!(validate_fetch_url("https://169.254.169.254/latest/meta-data").is_err());
    }

    #[test]
    fn search_args_are_bounded_and_default_to_five_results() {
        let args = validate_search_args(&json!({ "query": "  exa api  " })).unwrap();
        assert_eq!(args.query, "exa api");
        assert_eq!(args.count, 5);
        let args = validate_search_args(&json!({ "query": "exa", "num_results": 99 })).unwrap();
        assert_eq!(args.count, MAX_SEARCH_RESULTS);
        assert!(validate_search_args(&json!({ "query": "   " })).is_err());
        assert!(validate_search_args(&json!({ "query": "x".repeat(MAX_QUERY_CHARS + 1) })).is_err());
    }

    #[test]
    fn search_responses_keep_only_bounded_evidence() {
        let body = json!({
            "results": [
                {
                    "url": "https://example.com/a",
                    "title": " A ",
                    "publishedDate": "2026-01-01T00:00:00Z",
                    "highlights": ["first", "", "second", "third", "fourth"]
                },
                { "title": "missing url" }
            ]
        });
        assert!(parse_search_response(&body).is_err(), "a result without a URL must be rejected");
        let body = json!({
            "results": [
                {
                    "url": "https://example.com/a",
                    "title": " A ",
                    "publishedDate": "2026-01-01T00:00:00Z",
                    "highlights": ["first", "", "second", "third", "fourth"]
                }
            ]
        });
        let hits = parse_search_response(&body).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://example.com/a");
        assert_eq!(hits[0].title, "A");
        assert_eq!(hits[0].highlights, vec!["first", "second", "third"]);
    }

    #[test]
    fn search_api_keys_must_look_complete_without_storing_them_here() {
        assert!(validate_search_api_key("short").is_err());
        assert!(validate_search_api_key(&"x".repeat(257)).is_err());
        assert_eq!(validate_search_api_key("  exa-test-key  ").unwrap(), "exa-test-key");
    }

    #[cfg(windows)]
    #[test]
    fn runs_a_command_and_captures_stdout() {
        let mut cmd = build_command("echo", &argv(&["cameo-operator"]));
        cmd.current_dir(std::env::temp_dir());
        let out = run_command(cmd, Duration::from_secs(20)).expect("spawn");
        assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
        assert!(out.stdout.to_lowercase().contains("cameo-operator"), "stdout: {}", out.stdout);
        assert!(!out.timed_out);
    }

    #[test]
    fn windows_quoting_wraps_metacharacters() {
        assert_eq!(quote_win("cargo"), "cargo");
        assert_eq!(quote_win("a b"), "\"a b\"");
        assert_eq!(quote_win("a&b"), "\"a&b\"");
        assert_eq!(quote_win("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}
