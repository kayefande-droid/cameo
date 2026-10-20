use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, Wry};

fn pinned_default(app: &AppHandle<Wry>) -> bool {
    app.try_state::<crate::commands::store::AppState>()
        .and_then(|s| {
            let conn = s.db.lock().ok()?;
            conn.query_row(
                "SELECT value FROM prefs WHERE key = 'dock_pinned'",
                [],
                |r| r.get::<_, String>(0),
            )
            .ok()
        })
        .map(|v| v == "1")
        .unwrap_or(true)
}

/// Switch among three modes: top-center orb, compact floating dock, and full app.
#[tauri::command]
pub async fn set_window_mode(app: AppHandle<Wry>, mode: String) -> Result<(), String> {
    let win = app
        .get_webview_window("main")
        .ok_or("main window not found")?;

    let monitor = win
        .current_monitor()
        .map_err(|e| e.to_string())?
        .or_else(|| win.primary_monitor().ok().flatten());
    let (screen_width, screen_height, scale) = monitor
        .as_ref()
        .map(|monitor| {
            (
                monitor.size().width as f64 / monitor.scale_factor(),
                monitor.size().height as f64 / monitor.scale_factor(),
                monitor.scale_factor(),
            )
        })
        .unwrap_or((1280.0, 820.0, 1.0));
    let compact = mode != "full";
    let (width, height) = match mode.as_str() {
        "orb" => (72.0_f64.min(screen_width), 72.0_f64.min(screen_height)),
        // A compact window at startup; panel size adapts to the active monitor's work area.
        "dock" => (520.0_f64.min(screen_width), 76.0_f64.min(screen_height)),
        "panel" => (
            520.0_f64.min(screen_width),
            560.0_f64.min((screen_height - 16.0).max(240.0)),
        ),
        "full" => (1280.0_f64.min(screen_width), 820.0_f64.min(screen_height)),
        other => return Err(format!("unknown window mode '{other}'")),
    };
    let min_width = (if mode == "full" { 900.0_f64 } else { 360.0_f64 }).min(width);
    let min_height = (if mode == "panel" { 360.0_f64 } else if mode == "full" { 620.0_f64 } else { 68.0_f64 }).min(height);
    let logical = tauri::LogicalSize::new(width, height);
    let min = tauri::LogicalSize::new(min_width, min_height);
    let _ = scale;

    // Focusability is switched explicitly for the panel only. In idle dock mode,
    // Windows does not activate this topmost tool window when it is clicked.
    let _ = win.set_focusable(mode == "panel" || mode == "full");
    // Respect the saved pin preference in all compact modes.
    let _ = win.set_always_on_top(pinned_default(&app));
    let _ = win.set_skip_taskbar(compact);
    let _ = win.set_decorations(!compact);
    let _ = win.set_min_size(Some(min));
    if mode == "full" {
        let _ = win.set_max_size(None::<tauri::Size>);
    } else {
        let _ = win.set_max_size(Some(logical));
    }
    win.set_size(logical).map_err(|e| e.to_string())?;
    if mode == "panel" || mode == "full" {
        let _ = win.set_focus();
    }
    let _ = win.set_resizable(mode == "full");
    let _ = win.set_title(match mode.as_str() {
        "orb" => "CAMEO orb",
        "dock" => "CAMEO dock",
        _ => "CAMEO AI — Cognitive Autonomous Machine Executive Operator",
    });
    Ok(())
}

/// Center the dock horizontally near the top edge of the primary monitor,
/// leaving a small gap below the webcam/top bezel.
#[tauri::command]
pub async fn dock_reposition(app: AppHandle<Wry>) -> Result<(), String> {
    let win = app
        .get_webview_window("main")
        .ok_or("main window not found")?;
    let monitor = win
        .current_monitor()
        .map_err(|e| e.to_string())?
        .or_else(|| win.primary_monitor().ok().flatten())
        .ok_or("no current or primary monitor")?;
    let PhysicalSize { width, .. } = monitor.size();
    let origin = monitor.position();
    let scale = monitor.scale_factor();
    let mode = app
        .try_state::<crate::commands::store::AppState>()
        .and_then(|s| {
            let conn = s.db.lock().ok()?;
            conn.query_row(
                "SELECT value FROM prefs WHERE key = 'dock_mode'",
                [],
                |r| r.get::<_, String>(0),
            )
            .ok()
        })
        .unwrap_or_else(|| "dock".to_string());
    let logical_width = match mode.as_str() {
        "orb" => 72.0,
        "dock" => 520.0_f64.min(*width as f64 / scale),
        "panel" => 520.0_f64.min(*width as f64 / scale),
        "full" => 1280.0_f64.min(*width as f64 / scale),
        _ => 520.0_f64.min(*width as f64 / scale),
    };
    let logical_gap = match mode.as_str() {
        "panel" => 8.0,
        "dock" => 8.0,
        "orb" => 24.0,
        "full" => 8.0,
        _ => 8.0,
    };
    let (offset_x, offset_y) = app.try_state::<crate::commands::store::AppState>()
        .and_then(|s| {
            let conn = s.db.lock().ok()?;
            let read_offset = |key: &str, default: i32| conn.query_row(
                "SELECT value FROM prefs WHERE key = ?1", [key], |row| row.get::<_, String>(0),
            ).ok().and_then(|value| value.parse::<i32>().ok()).unwrap_or(default);
            Some((read_offset("dock_offset_x", 0).clamp(-600, 600), read_offset("dock_offset_y", 0).clamp(-500, 500)))
        })
        .unwrap_or((0, 0));
    let logical_height = match mode.as_str() {
        "orb" => 72.0,
        "dock" => 76.0,
        "panel" => 560.0_f64.min((monitor.size().height as f64 / scale - 16.0).max(240.0)),
        "full" => 820.0_f64.min(monitor.size().height as f64 / scale),
        _ => 76.0,
    };
    let outer_size = win.outer_size().ok();
    let win_width = outer_size
        .map(|size| size.width as i32)
        .unwrap_or_else(|| (logical_width * scale).round() as i32);
    let win_height = outer_size
        .map(|size| size.height as i32)
        .unwrap_or_else(|| (logical_height * scale).round() as i32);
    let y_gap = (logical_gap * scale).round() as i32;
    let monitor_width = *width as i32;
    let centered_x = ((monitor_width - win_width) / 2).max(0);
    let x = origin.x + (centered_x + (offset_x as f64 * scale).round() as i32).clamp(0, (monitor_width - win_width).max(0));
    let monitor_height = monitor.size().height as i32;
    let max_y = monitor_height - win_height;
    let y = origin.y + (y_gap + (offset_y as f64 * scale).round() as i32).clamp(0, max_y.max(0));
    win.set_position(PhysicalPosition::new(x, y))
        .map_err(|e| e.to_string())?;
    Ok(())
}
