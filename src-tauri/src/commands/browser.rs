use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder, Wry};

/// Open a URL in a dedicated child webview window. The window is fully
/// navigable by the user (address bar is in the app page); this command
/// merely creates the window.
#[tauri::command]
pub async fn open_browser_window(
    app: AppHandle<Wry>,
    url: String,
    title: Option<String>,
) -> Result<String, String> {
    // Only http(s) URLs are allowed.
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!("unsupported URL scheme in '{url}' — use http(s)"));
    }
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let label = format!("browser-{n}");
    let safe_title = title
        .map(|t| t.replace(['|', '"', '\\', '\n', '\r'], ""))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| url.clone());
    let url: tauri::Url = url
        .parse()
        .map_err(|e| format!("could not parse URL '{url}': {e}"))?;
    let win = WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(url))
        .title(format!("CAMEO Browser — {safe_title}"))
        .inner_size(1024.0, 768.0)
        .build()
        .map_err(|e| format!("failed to create browser window: {e}"))?;
    let _ = win.set_focus();
    Ok(label)
}
