// Windows power control via documented, user-facing mechanisms.
// IMPORTANT: these actions require the user's explicit confirmation in the UI.
// Real shutdowns/restarts are never triggered automatically (see tests plan).

use serde::{Deserialize, Serialize};

#[derive(Serialize, Clone, Debug)]
pub struct PowerResult {
    pub ok: bool,
    pub message: String,
    /// How the action was implemented, for transparency.
    pub mechanism: String,
    /// Whether a scheduled action can still be cancelled (shutdown only).
    pub cancellable: bool,
}

#[derive(Deserialize)]
pub struct PowerParams {
    pub action: String,
    /// Seconds of countdown before shutdown (shutdown/restart only).
    pub delay_seconds: u32,
    /// Must be true for shutdown/restart/sleep — the UI's confirmation gate.
    pub confirmed: bool,
}

fn run_win(tool: &str, args: &str) -> Result<String, String> {
    let out = std::process::Command::new(tool)
        .args(args.split_whitespace().collect::<Vec<_>>())
        .output()
        .map_err(|e| format!("failed to spawn {tool}: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
    .trim()
    .to_string();
    if out.status.success() {
        Ok(if text.is_empty() { "command succeeded".into() } else { text })
    } else {
        Err(if text.is_empty() {
            format!("{tool} exited with code {}", out.status.code().unwrap_or(-1))
        } else {
            text
        })
    }
}

#[tauri::command]
pub async fn power_action(params: PowerParams) -> Result<PowerResult, String> {
    if !params.confirmed && !matches!(params.action.as_str(), "cancel" | "status") {
        return Err("action not confirmed by the user — refusing to run".into());
    }
    match params.action.as_str() {
        "shutdown" => {
            if params.delay_seconds > 3600 {
                return Err("countdown too long (max 3600s)".into());
            }
            let delay = params.delay_seconds.max(5);
            let msg = run_win(
                "cmd",
                &format!("/C shutdown /s /t {delay} /c \"CAMEO scheduled shutdown\""),
            )?;
            Ok(PowerResult {
                ok: true,
                message: format!("Shutdown scheduled in {delay}s.\n{msg}\nUse Cancel to abort."),
                mechanism: "shutdown.exe /s /t <delay>".into(),
                cancellable: true,
            })
        }
        "restart" => {
            let delay = params.delay_seconds.max(5);
            let msg = run_win(
                "cmd",
                &format!("/C shutdown /r /t {delay} /c \"CAMEO scheduled restart\""),
            )?;
            Ok(PowerResult {
                ok: true,
                message: format!("Restart scheduled in {delay}s.\n{msg}"),
                mechanism: "shutdown.exe /r /t <delay>".into(),
                cancellable: true,
            })
        }
        "cancel" => {
            // Safe to call even when nothing is scheduled — replies with a warning.
            let msg = run_win("cmd", "/C shutdown /a").unwrap_or_else(|e| format!("nothing to cancel ({e})"));
            Ok(PowerResult {
                ok: true,
                message: format!("Shutdown cancellation requested.\n{msg}"),
                mechanism: "shutdown.exe /a".into(),
                cancellable: false,
            })
        }
        "sleep" => {
            let msg = run_win("rundll32", "powrprof.dll,SetSuspendState 0,1,0")
                .map_err(|e| format!("could not put the PC to sleep: {e}"))?;
            Ok(PowerResult {
                ok: true,
                message: format!("Sleep engaged.\n{msg}"),
                mechanism: "rundll32 powrprof.dll SetSuspendState".into(),
                cancellable: false,
            })
        }
        "lock" => {
            run_win("rundll32", "user32.dll,LockWorkStation")
                .map_err(|e| format!("could not lock the workstation: {e}"))?;
            Ok(PowerResult {
                ok: true,
                message: "Workstation lock requested.".into(),
                mechanism: "rundll32 user32.dll LockWorkStation".into(),
                cancellable: false,
            })
        }
        other => Err(format!("unknown action '{other}'")),
    }
}

#[derive(Serialize, Clone)]
pub struct ShutdownStatus {
    pub machine: String,
    pub os_version: String,
    pub waiting_for_amd64: Option<String>,
}

#[tauri::command]
pub async fn power_status() -> Result<ShutdownStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let os_version = std::process::Command::new("cmd")
            .args(["/C", "ver"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        let machine = std::process::Command::new("cmd")
            .args(["/C", "hostname"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        Ok(ShutdownStatus {
            machine,
            os_version,
            waiting_for_amd64: None,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}
