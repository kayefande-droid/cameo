use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{Manager, State};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

static RUN_SEQUENCE: AtomicU64 = AtomicU64::new(0);
/// Application state shared with all commands.
#[derive(Clone)]
pub struct AppState {
    /// SQLite connection. Secrets (API keys, proxy passwords) are NEVER stored here.
    pub db: Arc<Mutex<Connection>>,
    /// Cancellation flags keyed by request id, for in-flight chat streams.
    pub cancel_flags: Arc<Mutex<HashMap<String, Arc<std::sync::atomic::AtomicBool>>>>,
}

impl AppState {
    pub fn new(db: Connection) -> Self {
        Self {
            db: Arc::new(Mutex::new(db)),
            cancel_flags: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

fn with_db<T>(state: &AppState, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    f(&db).map_err(|e| e.to_string())
}

/// Open (creating if needed) the application database with its schema.
pub fn open_db(path: &std::path::Path) -> Connection {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(path).expect("failed to open cameo database");
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE IF NOT EXISTS prefs (
             key TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS tasks (
             id TEXT PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL DEFAULT '',
             agent TEXT NOT NULL DEFAULT 'cameo',
             status TEXT NOT NULL DEFAULT 'queued',
             current_step TEXT NOT NULL DEFAULT '',
             steps_json TEXT NOT NULL DEFAULT '[]',
             changed_files TEXT NOT NULL DEFAULT '[]',
             logs_json TEXT NOT NULL DEFAULT '[]',
             approval_id TEXT,
             project_root TEXT,
             created_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS file_proposals (
             id TEXT PRIMARY KEY,
             task_id TEXT NOT NULL,
             path TEXT NOT NULL,
             contents TEXT NOT NULL,
             original_contents TEXT,
             base_sha256 TEXT,
             status TEXT NOT NULL DEFAULT 'pending',
             proposed_at INTEGER NOT NULL,
             approved_at INTEGER,
             applied_at INTEGER
         );
         CREATE TABLE IF NOT EXISTS task_events (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             task_id TEXT NOT NULL,
             ts INTEGER NOT NULL,
             kind TEXT NOT NULL,
             message TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_task_events_task ON task_events(task_id);
         CREATE TABLE IF NOT EXISTS agent_runs (
             id TEXT PRIMARY KEY,
             task_id TEXT NOT NULL,
             agent TEXT NOT NULL,
             provider TEXT NOT NULL,
             model TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'queued',
             attempt INTEGER NOT NULL DEFAULT 0,
             result TEXT,
             error TEXT,
             created_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_agent_runs_task ON agent_runs(task_id, created_at);
         CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_runs_task_agent ON agent_runs(task_id, agent);
         CREATE TABLE IF NOT EXISTS bookmarks (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             title TEXT NOT NULL,
             url TEXT NOT NULL,
             created_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS proxy_profiles (
             id TEXT PRIMARY KEY,
             name TEXT NOT NULL,
             kind TEXT NOT NULL,
             host TEXT NOT NULL,
             port INTEGER NOT NULL,
             username TEXT,
             has_password INTEGER NOT NULL DEFAULT 0,
             enabled INTEGER NOT NULL DEFAULT 0,
             created_at INTEGER NOT NULL
         );",
    )
    .expect("failed to initialize cameo database schema");
    conn.execute_batch(MEMORY_SCHEMA_SQL).expect("failed to initialize memory schema");
    // Upgrade early prototype databases in place; CREATE TABLE does not add columns.
    let task_columns = conn
        .prepare("PRAGMA table_info(tasks)")
        .and_then(|mut stmt| {
            stmt.query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default();
    if !task_columns.iter().any(|c| c == "approval_id") {
        let _ = conn.execute("ALTER TABLE tasks ADD COLUMN approval_id TEXT", []);
    }
    if !task_columns.iter().any(|c| c == "project_root") {
        let _ = conn.execute("ALTER TABLE tasks ADD COLUMN project_root TEXT", []);
    }
    let proposal_columns = conn
        .prepare("PRAGMA table_info(file_proposals)")
        .and_then(|mut stmt| stmt.query_map([], |row| row.get::<_, String>(1))?.collect::<Result<Vec<_>, _>>())
        .unwrap_or_default();
    if !proposal_columns.iter().any(|c| c == "original_contents") {
        let _ = conn.execute("ALTER TABLE file_proposals ADD COLUMN original_contents TEXT", []);
    }
    conn
}

// ---------------------------------------------------------------------------
// Preferences
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn pref_get(key: String, state: State<'_, AppState>) -> Result<Option<String>, String> {
    with_db(&state, |db| {
        db.query_row("SELECT value FROM prefs WHERE key = ?1", [&key], |r| {
            r.get::<_, String>(0)
        })
        .optional()
    })
}

#[tauri::command]
pub fn pref_set(key: String, value: String, state: State<'_, AppState>) -> Result<(), String> {
    with_db(&state, |db| {
        db.execute(
            "INSERT INTO prefs (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            [&key, &value],
        )
        .map(|_| ())
    })
}

// ---------------------------------------------------------------------------
// Approved project memory. Data is CAMEO-owned; nothing here is sent to any
// provider. Values are bounded and secret-shaped text is redacted on save.
// ---------------------------------------------------------------------------

pub const MEMORY_SCHEMA_SQL: &str = "CREATE TABLE IF NOT EXISTS memory_entries (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    scope TEXT NOT NULL DEFAULT 'project',
    status TEXT NOT NULL DEFAULT 'active',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);";

pub const MEMORY_SIZE_LIMITS: (usize, usize) = (200, 20_000);

const MEMORY_KINDS: [&str; 7] = ["decision", "fact", "lesson", "runbook", "preference", "note", "unresolved-issue"];
const MEMORY_ACTIVE_STATUSES: [&str; 2] = ["active", "superseded"];

#[derive(Deserialize)]
pub struct MemoryInput {
    pub kind: String,
    pub title: String,
    pub body: String,
}

fn memory_json(row: &rusqlite::Row) -> rusqlite::Result<serde_json::Value> {
    Ok(serde_json::json!({
        "id": row.get::<_, String>(0)?,
        "kind": row.get::<_, String>(1)?,
        "title": row.get::<_, String>(2)?,
        "body": row.get::<_, String>(3)?,
        "scope": row.get::<_, String>(4)?,
        "status": row.get::<_, String>(5)?,
        "created_at": row.get::<_, i64>(6)?,
        "updated_at": row.get::<_, i64>(7)?,
    }))
}

fn validate_memory_input(input: &MemoryInput) -> Result<(String, String, String), String> {
    if !MEMORY_KINDS.contains(&input.kind.as_str()) {
        return Err(format!("memory kind must be one of: {}", MEMORY_KINDS.join(", ")));
    }
    let title = input.title.trim().to_string();
    let body = input.body.trim().to_string();
    let (title_max, body_max) = MEMORY_SIZE_LIMITS;
    if title.is_empty() || title.chars().count() > title_max {
        return Err(format!("memory title must be 1..={title_max} characters"));
    }
    if body.is_empty() || body.chars().count() > body_max {
        return Err(format!("memory body must be 1..={body_max} characters"));
    }
    Ok((input.kind.clone(), title, body))
}

fn memory_insert(db: &Connection, id: &str, kind: &str, title: &str, body: &str, now: i64) -> Result<(), String> {
    db.execute(
        "INSERT INTO memory_entries (id, kind, title, body, scope, status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 'project', 'active', ?5, ?5)",
        rusqlite::params![id, kind, title, body, now],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn memory_add(input: MemoryInput, state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let (kind, title, body) = validate_memory_input(&input)?;
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let id = format!("mem-{}-{}", chrono_now_ms(), RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed));
    let now = chrono_now_ms();
    memory_insert(&db, &id, &kind, &title, &body, now)?;
    db.query_row("SELECT id, kind, title, body, scope, status, created_at, updated_at FROM memory_entries WHERE id=?1", [&id], memory_json)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn memory_list(state: State<'_, AppState>) -> Result<Vec<serde_json::Value>, String> {
    with_db(&state, |db| {
        let mut stmt = db.prepare("SELECT id, kind, title, body, scope, status, created_at, updated_at FROM memory_entries ORDER BY updated_at DESC, id")?;
        let rows = stmt.query_map([], memory_json)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

#[tauri::command]
pub fn memory_update(id: String, title: Option<String>, body: Option<String>, status: Option<String>, state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    if !db.query_row("SELECT EXISTS(SELECT 1 FROM memory_entries WHERE id=?1)", [&id], |r| r.get::<_, bool>(0))
        .unwrap_or(false) {
        return Err("memory entry not found".into());
    }
    let now = chrono_now_ms();
    if let Some(title) = title {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > MEMORY_SIZE_LIMITS.0 {
            return Err(format!("memory title must be 1..={} characters", MEMORY_SIZE_LIMITS.0));
        }
        db.execute("UPDATE memory_entries SET title=?2, updated_at=?3 WHERE id=?1", rusqlite::params![id, title, now]).map_err(|e| e.to_string())?;
    }
    if let Some(body) = body {
        let body = body.trim();
        if body.is_empty() || body.chars().count() > MEMORY_SIZE_LIMITS.1 {
            return Err(format!("memory body must be 1..={} characters", MEMORY_SIZE_LIMITS.1));
        }
        db.execute("UPDATE memory_entries SET body=?2, updated_at=?3 WHERE id=?1", rusqlite::params![id, body, now]).map_err(|e| e.to_string())?;
    }
    if let Some(status) = status {
        if !MEMORY_ACTIVE_STATUSES.contains(&status.as_str()) {
            return Err("memory status must be active or superseded".into());
        }
        db.execute("UPDATE memory_entries SET status=?2, updated_at=?3 WHERE id=?1", rusqlite::params![id, status, now]).map_err(|e| e.to_string())?;
    }
    db.execute("UPDATE memory_entries SET updated_at=?2 WHERE id=?1 AND updated_at < ?2", rusqlite::params![id, now]).map_err(|e| e.to_string())?;
    db.query_row("SELECT id, kind, title, body, scope, status, created_at, updated_at FROM memory_entries WHERE id=?1", [&id], memory_json)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn memory_delete(id: String, state: State<'_, AppState>) -> Result<bool, String> {
    with_db(&state, |db| {
        db.execute("DELETE FROM memory_entries WHERE id=?1", [&id])
            .map(|n| n > 0)
    })
}

// ---------------------------------------------------------------------------
// Tasks + events
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct TaskRow {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub agent: String,
    pub status: String,
    #[serde(default)]
    pub current_step: String,
    #[serde(default)]
    pub steps_json: String,
    #[serde(default)]
    pub changed_files: String,
    #[serde(default)]
    pub logs_json: String,
    #[serde(default)]
    pub approval_id: Option<String>,
    #[serde(default)]
    pub project_root: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[tauri::command]
pub fn task_upsert(task: TaskRow, state: State<'_, AppState>) -> Result<(), String> {
    with_db(&state, |db| {
        db.execute(
            "INSERT INTO tasks (id, title, description, agent, status, current_step, steps_json, changed_files, logs_json, approval_id, project_root, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(id) DO UPDATE SET
               title=?2, description=?3, agent=?4, status=?5, current_step=?6,
               steps_json=?7, changed_files=?8, logs_json=?9, approval_id=?10, project_root=?11, created_at=?12, updated_at=?13",
            rusqlite::params![
                task.id,
                task.title,
                task.description,
                task.agent,
                task.status,
                task.current_step,
                task.steps_json,
                task.changed_files,
                task.logs_json,
                task.approval_id,
                task.project_root,
                task.created_at,
                task.updated_at
            ],
        )
        .map(|_| ())
    })
}

fn row_to_task(r: &rusqlite::Row) -> rusqlite::Result<serde_json::Value> {
    Ok(serde_json::json!({
        "id": r.get::<_, String>(0)?,
        "title": r.get::<_, String>(1)?,
        "description": r.get::<_, String>(2)?,
        "agent": r.get::<_, String>(3)?,
        "status": r.get::<_, String>(4)?,
        "current_step": r.get::<_, String>(5)?,
        "steps_json": r.get::<_, String>(6)?,
        "changed_files": r.get::<_, String>(7)?,
        "logs_json": r.get::<_, String>(8)?,
        "created_at": r.get::<_, i64>(9)?,
        "updated_at": r.get::<_, i64>(10)?,
        "approval_id": r.get::<_, Option<String>>(11)?,
        "project_root": r.get::<_, Option<String>>(12)?,
    }))
}

#[tauri::command]
pub fn task_list(state: State<'_, AppState>) -> Result<Vec<serde_json::Value>, String> {
    with_db(&state, |db| {
        let mut stmt = db.prepare(
            "SELECT id, title, description, agent, status, current_step, steps_json, changed_files, logs_json, created_at, updated_at, approval_id, project_root
             FROM tasks ORDER BY updated_at DESC",
        )?;
        let rows = stmt
            .query_map([], row_to_task)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

const PROJECT_CONTEXT_MAX_FILES: usize = 48;
const PROJECT_CONTEXT_MAX_BYTES: usize = 96 * 1024;
const PROJECT_CONTEXT_FILE_MAX_BYTES: usize = 16 * 1024;
const PROJECT_CONTEXT_PER_FILE_CHARS: usize = 12 * 1024;

fn context_file_allowed(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else { return false };
    let lower = name.to_ascii_lowercase();
    if lower.starts_with(".env") || ["secret", "credential", ".pem", ".key", ".p12", ".pfx", "id_rsa", "id_ed25519", "lock", "private", "token", "password", "passwd"].iter().any(|part| lower.contains(part)) {
        return false;
    }
    if ["package-lock.json", "pnpm-lock.yaml", "yarn.lock", "cargo.lock", "poetry.lock", "uv.lock"].contains(&lower.as_str()) {
        return false;
    }
    if ["config.json", "settings.json", "credentials.json"].contains(&lower.as_str()) { return false }
    matches!(path.extension().and_then(|ext| ext.to_str()).map(str::to_ascii_lowercase).as_deref(),
        Some("ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "rs" | "py" | "go" | "java" | "kt" | "kts" | "swift" | "c" | "h" | "cc" | "cpp" | "hpp" | "cs" | "php" | "rb" | "ex" | "exs" | "vue" | "svelte" | "html" | "css" | "scss" | "md" | "toml" | "yaml" | "yml" | "xml" | "sql" | "sh")
    )
}

fn collect_project_context(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if depth > 4 || out.len() >= PROJECT_CONTEXT_MAX_FILES { return Ok(()) }
    let mut entries = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot inspect project folder: {e}"))?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if out.len() >= PROJECT_CONTEXT_MAX_FILES { break }
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() { continue }
        if kind.is_dir() {
            if depth < 4 && !name.starts_with('.') && !["node_modules", "target", "dist", "build", "vendor", ".next", "coverage", "out"].contains(&name.as_str()) {
                collect_project_context(&path, depth + 1, out)?;
            }
        } else if kind.is_file() && context_file_allowed(&path) {
            out.push(path);
        }
    }
    Ok(())
}

/// Read a bounded, secret-filtered source snapshot from the task's pinned project folder.
#[tauri::command]
pub fn task_project_context(task_id: String, state: State<'_, AppState>) -> Result<Vec<serde_json::Value>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let root = task_project_root(&task_id, &db)?;
    drop(db);
    let mut paths = Vec::new();
    collect_project_context(&root, 0, &mut paths)?;
    let mut total_bytes = 0usize;
    let mut files = Vec::new();
    for path in paths {
        if total_bytes >= PROJECT_CONTEXT_MAX_BYTES { break }
        let metadata = match std::fs::metadata(&path) { Ok(metadata) => metadata, Err(_) => continue };
        if metadata.len() > PROJECT_CONTEXT_FILE_MAX_BYTES as u64 { continue }
        let contents = match std::fs::read_to_string(&path) { Ok(contents) => contents, Err(_) => continue };
        let remaining = PROJECT_CONTEXT_MAX_BYTES - total_bytes;
        let char_limit = remaining.min(PROJECT_CONTEXT_PER_FILE_CHARS);
        let limited = contents.chars().take(char_limit).collect::<String>();
        if limited.is_empty() { continue }
        total_bytes += limited.len();
        let relative = path.strip_prefix(&root).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
        files.push(serde_json::json!({ "path": relative, "contents": limited }));
    }
    Ok(files)
}

#[cfg(test)]
mod project_context_tests {
    use super::{context_file_allowed, START_REVIEW_RUN_SQL};
    use rusqlite::Connection;
    use std::path::Path;

    #[test]
    fn excludes_secrets_and_lockfiles_from_model_context() {
        for path in [".env", ".env.local", "secrets.json", "private_key.pem", "package-lock.json", "Cargo.lock", "credentials.json"] {
            assert!(!context_file_allowed(Path::new(path)), "must exclude {path}");
        }
    }

    #[test]
    fn includes_source_and_project_guidance_files() {
        for path in ["src/main.rs", "src/App.tsx", "README.md", "Cargo.toml"] {
            assert!(context_file_allowed(Path::new(path)), "must include {path}");
        }
    }

    #[test]
    fn memory_schema_and_visibility_filters_work_under_real_schema() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(super::MEMORY_SCHEMA_SQL).unwrap();
        db.execute("INSERT INTO memory_entries (id, kind, title, body, scope, status, created_at, updated_at) VALUES ('a','decision','Use task kinds','memory stays local','project','active',1,1), ('b','note','Old note','superseded content','project','superseded',1,1)", []).unwrap();
        let active: i64 = db.query_row("SELECT COUNT(*) FROM memory_entries WHERE status='active'", [], |r| r.get(0)).unwrap();
        assert_eq!(active, 1, "only active entries are served as normal recall");
        let entry: String = db.query_row("SELECT body FROM memory_entries WHERE id='a'", [], |r| r.get(0)).unwrap();
        assert_eq!(entry, "memory stays local");
    }

    #[test]
    fn memory_input_validation_rejects_unknown_kinds_and_oversized_titles() {
        let i = super::MemoryInput { kind: "not-a-kind".into(), title: "t".into(), body: "b".into() };
        assert!(super::validate_memory_input(&i).is_err(), "unknown kind must be rejected");
        let i = super::MemoryInput { kind: "fact".into(), title: " ".repeat(1000), body: "b".into() };
        assert!(super::validate_memory_input(&i).is_err(), "oversized title must be rejected");
        let i = super::MemoryInput { kind: "fact".into(), title: "ok".into(), body: "   ".into() };
        assert!(super::validate_memory_input(&i).is_err(), "blank body must be rejected");
        let long_ok = "x".repeat(200);
        let i = super::MemoryInput { kind: "fact".into(), title: long_ok.clone(), body: "b".into() };
        let (_, title, _) = super::validate_memory_input(&i).unwrap();
        assert_eq!(title, long_ok, "content at the limit must be accepted");
    }

    #[test]
    fn review_run_start_uses_atomic_database_cap_and_attempt_fence() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE agent_runs (id TEXT PRIMARY KEY, status TEXT NOT NULL, attempt INTEGER NOT NULL, error TEXT, updated_at INTEGER);").unwrap();
        db.execute("INSERT INTO agent_runs (id, status, attempt) VALUES ('active-1','running',1), ('active-2','running',1), ('queued-1','queued',0), ('exhausted','failed',2)", []).unwrap();
        let changed = db.execute(START_REVIEW_RUN_SQL, rusqlite::params!["queued-1", 1_i64]).unwrap();
        assert_eq!(changed, 0, "worker cap must hold in the database");
        db.execute("UPDATE agent_runs SET status='failed' WHERE id='active-1'", []).unwrap();
        let changed = db.execute(START_REVIEW_RUN_SQL, rusqlite::params!["queued-1", 2_i64]).unwrap();
        assert_eq!(changed, 1, "a freed slot should admit a queued worker");
        let changed = db.execute(START_REVIEW_RUN_SQL, rusqlite::params!["exhausted", 3_i64]).unwrap();
        assert_eq!(changed, 0, "attempt limit must be enforced in SQLite");
    }
}

#[tauri::command]
pub fn task_event_add(
    task_id: String,
    kind: String,
    message: String,
    state: State<'_, AppState>,
) -> Result<i64, String> {
    with_db(&state, |db| {
        db.execute(
            "INSERT INTO task_events (task_id, ts, kind, message) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![task_id, chrono_now_ms(), kind, message],
        )
        .map(|_| db.last_insert_rowid())
    })
}

fn chrono_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[tauri::command]
pub fn task_events(task_id: String, state: State<'_, AppState>) -> Result<Vec<serde_json::Value>, String> {
    with_db(&state, |db| {
        let mut stmt = db.prepare(
            "SELECT id, task_id, ts, kind, message FROM task_events WHERE task_id = ?1 ORDER BY ts ASC",
        )?;
        let rows = stmt
            .query_map([&task_id], |r| {
                Ok(serde_json::json!({
                    "id": r.get::<_, i64>(0)?,
                    "task_id": r.get::<_, String>(1)?,
                    "ts": r.get::<_, i64>(2)?,
                    "kind": r.get::<_, String>(3)?,
                    "message": r.get::<_, String>(4)?,
                }))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

#[tauri::command]
pub fn task_delete(task_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let mut db = state.db.lock().map_err(|e| e.to_string())?;
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let active: i64 = tx.query_row("SELECT COUNT(*) FROM agent_runs WHERE task_id=?1 AND status='running'", [&task_id], |r| r.get(0)).map_err(|e| e.to_string())?;
    if active > 0 { return Err("Cannot delete a task while a review worker is running. Cancel it first.".into()); }
    tx.execute("DELETE FROM task_events WHERE task_id = ?1", [&task_id]).map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM agent_runs WHERE task_id = ?1", [&task_id]).map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM file_proposals WHERE task_id = ?1", [&task_id]).map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM tasks WHERE id = ?1", [&task_id]).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// CAMEO-owned bounded specialist review runs.
// ---------------------------------------------------------------------------

const START_REVIEW_RUN_SQL: &str = "UPDATE agent_runs SET status='running', attempt=attempt+1, error=NULL, updated_at=?2 WHERE id=?1 AND status IN ('queued','failed','cancelled') AND attempt<2 AND (SELECT COUNT(*) FROM agent_runs WHERE status='running')<2";

#[derive(Deserialize)]
pub struct AgentRunInput {
    pub task_id: String,
    pub agent: String,
    pub provider: String,
    pub model: String,
}

fn agent_run_json(row: &rusqlite::Row) -> rusqlite::Result<serde_json::Value> {
    Ok(serde_json::json!({
        "id": row.get::<_, String>(0)?,
        "task_id": row.get::<_, String>(1)?,
        "agent": row.get::<_, String>(2)?,
        "provider": row.get::<_, String>(3)?,
        "model": row.get::<_, String>(4)?,
        "status": row.get::<_, String>(5)?,
        "attempt": row.get::<_, i64>(6)?,
        "result": row.get::<_, Option<String>>(7)?,
        "error": row.get::<_, Option<String>>(8)?,
        "created_at": row.get::<_, i64>(9)?,
        "updated_at": row.get::<_, i64>(10)?,
    }))
}

#[tauri::command]
pub fn agent_run_create(input: AgentRunInput, state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    if !["openrouter", "nvidia", "ollama", "compatible"].contains(&input.provider.as_str()) {
        return Err("unsupported CAMEO model provider".into());
    }
    if !["atlas", "forge", "sentinel"].contains(&input.agent.as_str()) {
        return Err("parallel review is limited to ATLAS, FORGE, and SENTINEL".into());
    }
    if input.task_id.trim().is_empty() || input.model.trim().is_empty() {
        return Err("task id and selected model are required".into());
    }
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let exists: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?1)", [&input.task_id], |r| r.get(0)).map_err(|e| e.to_string())?;
    if !exists { return Err("parent task not found".into()); }
    let id = format!("run-{}-{}-{}", chrono_now_ms(), std::process::id(), RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed));
    let now = chrono_now_ms();
    db.execute("INSERT INTO agent_runs (id, task_id, agent, provider, model, status, attempt, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 'queued', 0, ?6, ?6)", rusqlite::params![id, input.task_id, input.agent, input.provider, input.model, now]).map_err(|e| e.to_string())?;
    db.execute("INSERT INTO task_events (task_id, ts, kind, message) VALUES (?1, ?2, 'agent_run_queued', ?3)", rusqlite::params![input.task_id, now, format!("{} review worker queued", input.agent.to_uppercase())]).map_err(|e| e.to_string())?;
    db.query_row("SELECT id, task_id, agent, provider, model, status, attempt, result, error, created_at, updated_at FROM agent_runs WHERE id=?1", [&id], agent_run_json).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn agent_run_list(task_id: String, state: State<'_, AppState>) -> Result<Vec<serde_json::Value>, String> {
    with_db(&state, |db| {
        let mut stmt = db.prepare("SELECT id, task_id, agent, provider, model, status, attempt, result, error, created_at, updated_at FROM agent_runs WHERE task_id=?1 ORDER BY created_at, id")?;
        stmt.query_map([task_id], agent_run_json)?.collect::<Result<Vec<_>, _>>()
    })
}

#[tauri::command]
pub fn agent_run_start(run_id: String, state: State<'_, AppState>) -> Result<Option<i64>, String> {
    let mut db = state.db.lock().map_err(|e| e.to_string())?;
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let now = chrono_now_ms();
    let changed = tx.execute(START_REVIEW_RUN_SQL, rusqlite::params![run_id, now]).map_err(|e| e.to_string())?;
    if changed == 0 { return Ok(None); }
    let (task_id, agent, attempt): (String, String, i64) = tx.query_row("SELECT task_id, agent, attempt FROM agent_runs WHERE id=?1", [&run_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(|e| e.to_string())?;
    tx.execute("INSERT INTO task_events (task_id, ts, kind, message) VALUES (?1, ?2, 'agent_run_started', ?3)", rusqlite::params![task_id, now, format!("{} review worker started (attempt {attempt}/2)", agent.to_uppercase())]).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(Some(attempt))
}

#[tauri::command]
pub fn agent_run_finish(run_id: String, attempt: i64, status: String, result: Option<String>, error: Option<String>, state: State<'_, AppState>) -> Result<(), String> {
    if !["completed", "failed", "cancelled"].contains(&status.as_str()) { return Err("invalid terminal agent run status".into()); }
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let (task_id, agent): (String, String) = db.query_row("SELECT task_id, agent FROM agent_runs WHERE id=?1", [&run_id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?;
    let now = chrono_now_ms();
    let safe_result = result.map(|value| value.chars().take(24_000).collect::<String>());
    let safe_error = error.map(|value| value.chars().take(1000).collect::<String>());
    let changed = db.execute("UPDATE agent_runs SET status=?2, result=?3, error=?4, updated_at=?5 WHERE id=?1 AND status='running' AND attempt=?6", rusqlite::params![run_id, status, safe_result, safe_error, now, attempt]).map_err(|e| e.to_string())?;
    if changed == 0 { return Err("agent run is not active".into()); }
    db.execute("INSERT INTO task_events (task_id, ts, kind, message) VALUES (?1, ?2, ?3, ?4)", rusqlite::params![task_id, now, format!("agent_run_{status}"), format!("{} review worker {status}", agent.to_uppercase())]).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn agent_run_cancel(run_id: String, attempt: Option<i64>, state: State<'_, AppState>) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let row: Option<(String, String)> = db.query_row("SELECT task_id, agent FROM agent_runs WHERE id=?1 AND status IN ('queued','running') AND (?2 IS NULL OR attempt=?2)", rusqlite::params![run_id, attempt], |r| Ok((r.get(0)?, r.get(1)?))).optional().map_err(|e| e.to_string())?;
    let Some((task_id, agent)) = row else { return Ok(()); };
    let now = chrono_now_ms();
    db.execute("UPDATE agent_runs SET status='cancelled', error='Cancelled by user.', updated_at=?2 WHERE id=?1 AND status IN ('queued','running') AND (?3 IS NULL OR attempt=?3)", rusqlite::params![run_id, now, attempt]).map_err(|e| e.to_string())?;
    db.execute("INSERT INTO task_events (task_id, ts, kind, message) VALUES (?1, ?2, 'agent_run_cancelled', ?3)", rusqlite::params![task_id, now, format!("{} review worker cancelled", agent.to_uppercase())]).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn agent_run_recover_expired(task_id: String, state: State<'_, AppState>) -> Result<i64, String> {
    let mut db = state.db.lock().map_err(|e| e.to_string())?;
    let tx = db.transaction().map_err(|e| e.to_string())?;
    let cutoff = chrono_now_ms() - 120_000;
    let mut stmt = tx.prepare("SELECT id, agent, attempt FROM agent_runs WHERE task_id=?1 AND status='running' AND updated_at<=?2").map_err(|e| e.to_string())?;
    let expired = stmt.query_map(rusqlite::params![task_id, cutoff], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))
        .map_err(|e| e.to_string())?.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    drop(stmt);
    let mut recovered = 0;
    for (id, agent, attempt) in expired {
        let now = chrono_now_ms();
        let message = format!("{} review worker exceeded its 120-second deadline and was marked interrupted; attempt {attempt} remains available for retry.", agent.to_uppercase());
        let changed = tx.execute("UPDATE agent_runs SET status='failed', error=?2, updated_at=?3 WHERE id=?1 AND status='running' AND attempt=?4 AND updated_at<=?5", rusqlite::params![id, message, now, attempt, cutoff]).map_err(|e| e.to_string())?;
        if changed > 0 {
            tx.execute("INSERT INTO task_events (task_id, ts, kind, message) VALUES (?1, ?2, 'agent_run_recovered', ?3)", rusqlite::params![task_id, now, message]).map_err(|e| e.to_string())?;
            recovered += 1;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(recovered)
}

// ---------------------------------------------------------------------------
// User-reviewed file proposals. Model output is inert until a direct user
// confirmation calls task_apply_proposal; project roots are pinned per task.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ProposedFileInput {
    pub path: String,
    pub contents: String,
}

#[derive(Serialize)]
pub struct FileProposal {
    pub id: String,
    pub task_id: String,
    pub path: String,
    pub original_contents: Option<String>,
    pub contents: String,
    pub status: String,
    pub base_sha256: Option<String>,
    pub backup_path: Option<String>,
}

fn safe_relative_path(path: &str) -> Result<&Path, String> {
    let p = Path::new(path);
    if path.trim().is_empty() || p.is_absolute() {
        return Err("proposal path must be a non-empty relative path".into());
    }
    if p.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err("proposal path may not contain '.', '..', root, or prefix components".into());
    }
    Ok(p)
}

fn sha256(contents: &[u8]) -> String {
    format!("{:x}", Sha256::digest(contents))
}

fn task_project_root(task_id: &str, db: &Connection) -> Result<PathBuf, String> {
    let root: Option<String> = db
        .query_row("SELECT project_root FROM tasks WHERE id=?1", [task_id], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?
        .flatten();
    let root = root.ok_or_else(|| "task has no selected project folder".to_string())?;
    PathBuf::from(root).canonicalize().map_err(|e| format!("project root is unavailable: {e}"))
}

fn checked_target(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    let joined = root.join(relative);
    let parent = joined.parent().ok_or("proposal must name a file inside the project")?;
    let canonical_parent = parent.canonicalize().map_err(|e| format!("parent folder must already exist: {e}"))?;
    if !canonical_parent.starts_with(root) {
        return Err("proposal resolves outside the selected project folder".into());
    }
    let target = canonical_parent.join(joined.file_name().ok_or("invalid proposal filename")?);
    if std::fs::symlink_metadata(&target).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err("symlink targets are not supported for file proposals".into());
    }
    if target.exists() {
        let canonical_target = target.canonicalize().map_err(|e| e.to_string())?;
        if !canonical_target.starts_with(root) || !canonical_target.is_file() {
            return Err("proposal target is not a regular file inside the selected project".into());
        }
        Ok(canonical_target)
    } else {
        Ok(target)
    }
}

#[tauri::command]
pub fn task_propose_files(
    task_id: String,
    files: Vec<ProposedFileInput>,
    state: State<'_, AppState>,
) -> Result<Vec<FileProposal>, String> {
    if files.is_empty() || files.len() > 12 {
        return Err("a proposal must contain between 1 and 12 files".into());
    }
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let root = task_project_root(&task_id, &db)?;
    let mut seen_paths = std::collections::HashSet::new();
    static PROPOSAL_ID: AtomicU64 = AtomicU64::new(0);
    let mut created = Vec::new();
    for file in files.into_iter() {
        if file.contents.len() > 1_000_000 {
            return Err(format!("proposal file '{}' exceeds 1 MB", file.path));
        }
        let relative = safe_relative_path(&file.path)?;
        let normalized = relative.to_string_lossy().replace('\\', "/");
        if !seen_paths.insert(normalized) {
            return Err(format!("proposal contains duplicate target path '{}'", file.path));
        }
        let target = checked_target(&root, relative)?;
        let metadata = std::fs::symlink_metadata(&target);
        if metadata.as_ref().is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!("symlink targets are not supported for proposal '{}'; use a regular in-project file", file.path));
        }
        let original = if target.exists() {
            Some(std::fs::read_to_string(&target).map_err(|e| format!("cannot preview '{}': {e}", file.path))?)
        } else {
            None
        };
        let digest = original.as_ref().map(|s| sha256(s.as_bytes()));
        let id = format!("p-{}-{}-{}", chrono_now_ms(), std::process::id(), PROPOSAL_ID.fetch_add(1, Ordering::Relaxed));
        db.execute(
            "INSERT INTO file_proposals (id, task_id, path, contents, original_contents, base_sha256, status, proposed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7)",
            rusqlite::params![id, task_id, file.path, file.contents, original, digest, chrono_now_ms()],
        ).map_err(|e| e.to_string())?;
        created.push(id);
    }
    let mut out = Vec::new();
    for id in created {
        let proposal = db.query_row(
            "SELECT id, task_id, path, original_contents, contents, status, base_sha256 FROM file_proposals WHERE id=?1",
            [&id],
            |r| Ok(FileProposal {
                id: r.get(0)?, task_id: r.get(1)?, path: r.get(2)?, original_contents: r.get(3)?, contents: r.get(4)?, status: r.get(5)?, base_sha256: r.get(6)?, backup_path: None,
            }),
        ).map_err(|e| e.to_string())?;
        out.push(proposal);
    }
    Ok(out)
}

#[tauri::command]
pub fn task_file_proposals(task_id: String, state: State<'_, AppState>) -> Result<Vec<FileProposal>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let mut stmt = db.prepare("SELECT id, task_id, path, original_contents, contents, status, base_sha256 FROM file_proposals WHERE task_id=?1 ORDER BY proposed_at, id")
        .map_err(|e| e.to_string())?;
    stmt.query_map([task_id], |r| Ok(FileProposal {
        id: r.get(0)?, task_id: r.get(1)?, path: r.get(2)?, original_contents: r.get(3)?, contents: r.get(4)?, status: r.get(5)?, base_sha256: r.get(6)?, backup_path: None,
    }))
    .map_err(|e| e.to_string())?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| e.to_string())
}

fn refresh_task_proposal_status(db: &Connection, task_id: &str) -> Result<(), String> {
    let (pending, applied, rejected): (i64, i64, i64) = db.query_row(
        "SELECT SUM(CASE WHEN status='pending' THEN 1 ELSE 0 END), SUM(CASE WHEN status='applied' THEN 1 ELSE 0 END), SUM(CASE WHEN status='rejected' THEN 1 ELSE 0 END) FROM file_proposals WHERE task_id=?1",
        [task_id],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
    ).map_err(|e| e.to_string())?;
    if pending > 0 { return Ok(()) }
    if applied == 0 && rejected == 0 { return Ok(()) }
    let (status, current_step) = if applied == 0 {
        ("cancelled", "All proposed file changes were rejected".to_string())
    } else if rejected > 0 {
        ("partially_completed", format!("Proposal review finished: {applied} applied, {rejected} rejected; checks not run"))
    } else {
        // Writing approved edits is not verification; keep the task in an explicit review state.
        ("awaiting_verification", "Approved edits applied; checks not run".to_string())
    };
    db.execute("UPDATE tasks SET status=?2, current_step=?3, updated_at=?4 WHERE id=?1", rusqlite::params![task_id, status, current_step, chrono_now_ms()]).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn task_reject_proposal(proposal_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let task_id: String = db.query_row("SELECT task_id FROM file_proposals WHERE id=?1 AND status='pending'", [&proposal_id], |row| row.get(0))
        .map_err(|_| "proposal not found or no longer pending".to_string())?;
    let n = db.execute("UPDATE file_proposals SET status='rejected' WHERE id=?1 AND status='pending'", [&proposal_id]).map_err(|e| e.to_string())?;
    if n == 0 { return Err("proposal not found or no longer pending".into()); }
    refresh_task_proposal_status(&db, &task_id)
}

#[tauri::command]
pub fn task_apply_proposal(
    proposal_id: String,
    confirmed: bool,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<FileProposal, String> {
    if !confirmed {
        return Err("file changes require explicit user approval".into());
    }
    let (task_id, relative, contents, original, digest) = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        db.query_row(
            "SELECT task_id, path, contents, original_contents, base_sha256 FROM file_proposals WHERE id=?1 AND status='pending'",
            [&proposal_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?)),
        ).map_err(|_| "proposal not found or no longer pending".to_string())?
    };
    let root = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        task_project_root(&task_id, &db)?
    };
    let relative_path = safe_relative_path(&relative)?;
    let target = checked_target(&root, relative_path)?;
    let current_metadata = std::fs::symlink_metadata(&target).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound { "target does not exist".to_string() } else { e.to_string() }
    });
    if let Ok(metadata) = &current_metadata {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("proposal target must be a regular non-symlink file".into());
        }
    }
    let current = if target.exists() {
        Some(std::fs::read(&target).map_err(|e| format!("cannot read current file: {e}"))?)
    } else { None };
    let current_hash = current.as_ref().map(|b| sha256(b));
    if current_hash != digest {
        return Err("file changed after the proposal was prepared. Reject it and ask the specialist to re-read the latest file before proposing again.".into());
    }
    let backup = if let Some(bytes) = current.as_ref() {
        let mut dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
        dir.push("backups");
        std::fs::create_dir_all(&dir).map_err(|e| format!("could not create backup folder: {e}"))?;
        dir.push(format!("{proposal_id}.bak"));
        std::fs::write(&dir, bytes).map_err(|e| format!("could not save backup; file not changed: {e}"))?;
        Some(dir)
    } else { None };
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    if current.is_some() {
        options.truncate(true);
    } else {
        options.create_new(true);
    }
    use std::io::Write;
    let mut output = options.open(&target).map_err(|e| format!("target changed or could not be opened safely: {e}"))?;
    output.write_all(contents.as_bytes()).map_err(|e| format!("write failed: {e}"))?;
    let backup_path = backup.map(|p| p.display().to_string());
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.execute("UPDATE file_proposals SET status='applied', applied_at=?2 WHERE id=?1 AND status='pending'", rusqlite::params![proposal_id, chrono_now_ms()]).map_err(|e| e.to_string())?;
    let mut files: String = db.query_row("SELECT changed_files FROM tasks WHERE id=?1", [&task_id], |r| r.get(0)).unwrap_or_else(|_| "[]".into());
    let mut parsed: Vec<String> = serde_json::from_str(&files).unwrap_or_default();
    parsed.push(relative.clone());
    files = serde_json::to_string(&parsed).map_err(|e| e.to_string())?;
    db.execute("UPDATE tasks SET changed_files=?2, updated_at=?3 WHERE id=?1", rusqlite::params![task_id, files, chrono_now_ms()]).map_err(|e| e.to_string())?;
    refresh_task_proposal_status(&db, &task_id)?;
    db.execute("INSERT INTO task_events (task_id, ts, kind, message) VALUES (?1, ?2, 'file_applied', ?3)", rusqlite::params![task_id, chrono_now_ms(), format!("Applied approved change to {relative}; backup: {}", backup_path.as_deref().unwrap_or("not needed for new file"))]).map_err(|e| e.to_string())?;
    Ok(FileProposal { id: proposal_id, task_id, path: relative, original_contents: original, contents, status: "applied".into(), base_sha256: digest, backup_path })
}

// ---------------------------------------------------------------------------
// Proxy profiles
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ProxyProfileRow {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub host: String,
    pub port: i64,
    pub username: Option<String>,
    pub has_password: i64,
    pub enabled: i64,
}

fn row_to_proxy(r: &rusqlite::Row) -> rusqlite::Result<serde_json::Value> {
    Ok(serde_json::json!({
        "id": r.get::<_, String>(0)?,
        "name": r.get::<_, String>(1)?,
        "kind": r.get::<_, String>(2)?,
        "host": r.get::<_, String>(3)?,
        "port": r.get::<_, i64>(4)?,
        "username": r.get::<_, Option<String>>(5)?,
        "has_password": r.get::<_, i64>(6)?,
        "enabled": r.get::<_, i64>(7)?,
    }))
}

#[tauri::command]
pub fn proxy_save(profile: ProxyProfileRow, password: Option<String>, state: State<'_, AppState>) -> Result<(), String> {
    if !["http", "https", "socks5"].contains(&profile.kind.as_str()) {
        return Err("proxy kind must be http, https or socks5".into());
    }
    if !(0 < profile.port && profile.port < 65536) {
        return Err("proxy port must be in 1..65535".into());
    }
    if profile.host.trim().is_empty() {
        return Err("proxy host must not be empty".into());
    }
    //Passwords go to the OS credential store, never SQLite.
    let pw = password.unwrap_or_default();
    let has_password = if pw.is_empty() { profile.has_password } else { 1 };
    with_db(&state, |db| {
        db.execute(
            "INSERT INTO proxy_profiles (id, name, kind, host, port, username, has_password, enabled, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(id) DO UPDATE SET name=?2, kind=?3, host=?4, port=?5, username=?6, has_password=?7, enabled=?8",
            rusqlite::params![
                profile.id, profile.name, profile.kind, profile.host.trim(),
                profile.port, profile.username, has_password, profile.enabled, chrono_now_ms()
            ],
        )
        .map(|_| ())
    })?;
    if !pw.is_empty() {
        let entry = keyring::Entry::new(&format!("cameo_proxy_{}", profile.id), "password");
        if let Ok(entry) = entry {
            if let Err(e) = entry.set_password(&pw) {
                return Err(format!("proxy saved but password could not be stored securely: {e}"));
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub fn proxy_list(state: State<'_, AppState>) -> Result<Vec<serde_json::Value>, String> {
    with_db(&state, |db| {
        let mut stmt = db.prepare(
            "SELECT id, name, kind, host, port, username, has_password, enabled FROM proxy_profiles ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], row_to_proxy)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

#[tauri::command]
pub fn proxy_delete(id: String, state: State<'_, AppState>) -> Result<(), String> {
    with_db(&state, |db| {
        db.execute("DELETE FROM proxy_profiles WHERE id = ?1", [&id]).map(|_| ())
    })
}

// ---------------------------------------------------------------------------
// Proxy connection test
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, Clone)]
pub struct ProxyTestResult {
    pub ok: bool,
    pub message: String,
    pub via_proxy: bool,
    pub elapsed_ms: Option<u64>,
    pub egress_ip: Option<String>,
}

fn proxy_url_for(pid: &str, db: &Connection) -> Result<Option<String>, String> {
    if pid.is_empty() {
        return Ok(None);
    }
    let row = db.query_row(
        "SELECT kind, host, port, username, has_password FROM proxy_profiles WHERE id = ?1",
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
    );
    let Ok((kind, host, port, username, has_password)) = row else {
        return Ok(None);
    };
    let scheme = match kind.as_str() {
        "socks5" => "socks5",
        "https" => "https",
        _ => "http",
    };
    let mut url = format!("{scheme}://{host}:{port}");
    if let Some(user) = username.filter(|u| !u.is_empty()) {
        let secret: String = if has_password == 1 {
            keyring::Entry::new(&format!("cameo_proxy_{pid}"), "password")
                .ok()
                .and_then(|e| e.get_password().ok())
                .unwrap_or_default()
        } else {
            String::new()
        };
        url = format!("{scheme}://{user}:{secret}@{host}:{port}");
    }
    Ok(Some(url))
}

#[tauri::command]
pub async fn proxy_test(profile_id: String, state: State<'_, AppState>) -> Result<ProxyTestResult, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let url = {
            let db = state.db.lock().map_err(|e| e.to_string())?;
            proxy_url_for(&profile_id, &db)?
        };
        let client = tauri::async_runtime::block_on(async {
            let mut b = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .user_agent("CAMEO-Proxy-Test/0.1");
            if let Some(u) = url {
                b = b.proxy(reqwest::Proxy::all(&u).map_err(|e| format!("invalid proxy URL: {e}"))?);
            }
            b.build().map_err(|e| format!("client error: {e}"))
        })?;
        let start = std::time::Instant::now();
        let resp = tauri::async_runtime::block_on(async {
            client
                .get("https://api.ipify.org?format=json")
                .send()
                .await
                .map_err(|e| format!("connection through the proxy failed: {e}"))
        });
        let elapsed = start.elapsed().as_millis() as u64;
        match resp {
            Ok(r) => {
                let status = r.status().as_u16();
                let body = tauri::async_runtime::block_on(async { r.text().await.unwrap_or_default() });
                let ip = serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| v["ip"].as_str().map(|s| s.to_string()));
                if status == 200 {
                    Ok(ProxyTestResult {
                        ok: true,
                        message: if profile_id.is_empty() { "direct connection OK".into() } else { "proxy connection OK".into() },
                        via_proxy: !profile_id.is_empty(),
                        elapsed_ms: Some(elapsed),
                        egress_ip: ip,
                    })
                } else {
                    Ok(ProxyTestResult {
                        ok: false,
                        message: format!("proxy reachable but endpoint returned HTTP {status}"),
                        via_proxy: !profile_id.is_empty(),
                        elapsed_ms: Some(elapsed),
                        egress_ip: None,
                    })
                }
            }
            Err(e) => Ok(ProxyTestResult {
                ok: false,
                message: e,
                via_proxy: !profile_id.is_empty(),
                elapsed_ms: Some(elapsed),
                egress_ip: None,
            }),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn db_ping() -> String {
    "ok".into()
}
