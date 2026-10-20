import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface OpenResult {
  label: string;
  title: string;
}

export default function Browser() {
  const [url, setUrl] = useState("https://cameo.local/");
  const [tabs, setTabs] = useState<OpenResult[]>([]);
  const [error, setError] = useState<string | null>(null);

  const openInBrowserWindow = async () => {
    setError(null);
    let normalized = url.trim();
    if (!/^https?:\/\//i.test(normalized)) {
      if (/\s/.test(normalized) || !normalized.includes(".")) {
        normalized = `https://duckduckgo.com/?q=${encodeURIComponent(normalized)}`;
      } else {
        normalized = `https://${normalized}`;
      }
    }
    if (!/^https?:\/\//i.test(normalized)) {
      setError("Only http(s) URLs are supported.");
      return;
    }
    try {
      const title =
        normalized.replace(/^https?:\/\//, "").split("/")[0] || normalized;
      const label = await invoke<string>("open_browser_window", {
        url: normalized,
        title,
      });
      setTabs((t) => [...t, { label, title }]);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <>
      <div className="card">
        <h3>
          <span className="section-title">Open a website</span>
        </h3>
        <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
          <input
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void openInBrowserWindow();
            }}
            aria-label="URL or search query"
            className="mono"
            placeholder="https://example.com or a search query"
          />
          <button className="btn primary" onClick={openInBrowserWindow}>
            Open window
          </button>
        </div>
        {error && <div className="note danger">{error}</div>}
        {tabs.length === 0 ? (
          <p className="note" style={{ marginTop: 10 }}>
            Browsing happens in a real app-managed webview window — no third-party browser is launched.
          </p>
        ) : (
          <>
            <p className="note" style={{ marginTop: 10 }}>Open browser windows (real webviews):</p>
            {tabs.map((t) => (
              <div key={t.label} className="note mono" style={{ padding: "4px 0" }}>
                · {t.title} ({t.label})
              </div>
            ))}
          </>
        )}
      </div>
      <div className="card">
        <h3>Scope &amp; limitations</h3>
        <ul className="note" style={{ paddingLeft: 18, marginTop: 6, lineHeight: 1.7 }}>
          <li>Each window is its own webview; tabs read as separate real windows.</li>
          <li>Downloads, bookmarks and history hit Tauri webview limitations and are not claimed as working yet.</li>
          <li>Browser automation is disabled by default — enabling it requires your explicit approval.</li>
        </ul>
      </div>
    </>
  );
}
