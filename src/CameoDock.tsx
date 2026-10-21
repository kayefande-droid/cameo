import { useEffect, useId, useState } from "react";
import type { ReactNode } from "react";
import "./CameoDock.css";

export type CameoDockMode = "orb" | "dock" | "workspace";
export type CameoDockPage =
  | "Assistant"
  | "Tasks"
  | "Agents"
  | "Developer"
  | "Browser"
  | "Voice"
  | "Settings"
  | "Connections"
  | "Proxy"
  | "Camera"
  | "Power"
  | "About";

export interface CameoDockProps {
  /** Connect these callbacks to the existing CAMEO app state/window commands. */
  initialMode?: CameoDockMode;
  initialPage?: CameoDockPage;
  pinned?: boolean;
  onModeChange?: (mode: CameoDockMode) => void;
  onNavigate?: (page: CameoDockPage) => void;
  onPinToggle?: (pinned: boolean) => void;
  onSnapToTop?: () => void;
  /** Real submission handler: when provided, the composer sends through it.
   *  Without it the composer stays a UI-only preview. */
  onSubmitPrompt?: (text: string) => void;
  /** Existing app content for the expanded panel. */
  children?: ReactNode;
  /** Set false if the existing app already owns window dragging. */
  draggable?: boolean;
}

const NAV_GROUPS: { title: string; items: CameoDockPage[] }[] = [
  { title: "Operate", items: ["Assistant", "Tasks", "Agents"] },
  { title: "Build", items: ["Developer", "Browser", "Camera", "Voice"] },
  { title: "System", items: ["Connections", "Proxy", "Power", "Settings", "About"] },
];

const pages: { label: CameoDockPage; glyph: string; hint: string }[] = [
  { label: "Assistant", glyph: "✳", hint: "Chat with CAMEO" },
  { label: "Tasks", glyph: "▤", hint: "Projects and tasks" },
  { label: "Agents", glyph: "◈", hint: "Specialist agents" },
  { label: "Developer", glyph: "</>", hint: "Developer tools" },
  { label: "Browser", glyph: "◎", hint: "Web workspace" },
  { label: "Voice", glyph: "♫", hint: "Voice settings" },
  { label: "Settings", glyph: "⚙", hint: "Preferences" },
  { label: "Connections", glyph: "⇄", hint: "Providers and model discovery" },
  { label: "Proxy", glyph: "◫", hint: "Proxy profiles and tests" },
  { label: "Camera", glyph: "◉", hint: "Camera preview" },
  { label: "Power", glyph: "⏻", hint: "Power controls" },
  { label: "About", glyph: "ⓘ", hint: "About CAMEO" },
];

export default function CameoDock({
  initialMode = "dock",
  initialPage = "Assistant",
  pinned: controlledPinned,
  onModeChange,
  onNavigate,
  onPinToggle,
  onSnapToTop,
  onSubmitPrompt,
  children,
  draggable = true,
}: CameoDockProps) {
  const [mode, setMode] = useState<CameoDockMode>(initialMode);
  const [page, setPage] = useState<CameoDockPage>(initialPage);
  const [localPinned, setLocalPinned] = useState(true);
  const [menuOpen, setMenuOpen] = useState(false);
  const [prompt, setPrompt] = useState("");
  const titleId = useId();
  const pinned = controlledPinned ?? localPinned;

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && menuOpen) setMenuOpen(false);
      if (event.altKey && event.key === "ArrowDown") changeMode("dock");
      if (event.altKey && event.key === "ArrowUp") changeMode("orb");
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
    // changeMode is intentionally stable enough for this keyboard binding.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [menuOpen]);

  function changeMode(next: CameoDockMode) {
    setMode(next);
    onModeChange?.(next);
  }

  function navigate(next: CameoDockPage) {
    setPage(next);
    setMenuOpen(false);
    onNavigate?.(next);
  }

  function togglePinned() {
    const next = !pinned;
    if (controlledPinned === undefined) setLocalPinned(next);
    onPinToggle?.(next);
  }

  function submitPrompt(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const text = prompt.trim();
    // Connected phase: the host app owns real submission through onSubmitPrompt.
    // Without a handler this intentionally stays a UI-only preview.
    if (text && onSubmitPrompt) onSubmitPrompt(text);
    setPrompt("");
  }

  return (
    <main className={`cameo-stage cameo-mode-${mode}`} aria-label="CAMEO dock preview">
      <section className={`cameo-dock ${mode === "orb" ? "is-orb" : ""} ${mode === "workspace" ? "is-workspace" : ""}`}
        aria-labelledby={titleId}>
        <header className="cameo-pill" data-tauri-drag-region={draggable ? "" : undefined}>
          <button
            className="cameo-brand"
            type="button"
            aria-label={mode === "orb" ? "Expand CAMEO dock" : "CAMEO home"}
            title="CAMEO AI"
            onClick={() => changeMode(mode === "orb" ? "dock" : "workspace")}
          >
            <span className="cameo-mark" aria-hidden="true"><img src="/logo.svg" alt="" /></span>
            <span className="cameo-brand-copy">
              <strong id={titleId}>CAMEO</strong>
              <small>AI OPERATOR</small>
            </span>
          </button>

          {mode !== "orb" && (
            <>
              <span className="cameo-separator" aria-hidden="true" />
              <button className="cameo-current-page" type="button" onClick={() => setMenuOpen(!menuOpen)}
                aria-expanded={menuOpen} aria-haspopup="menu">
                <span className="cameo-current-glyph">{pages.find(p => p.label === page)?.glyph}</span>
                <span>{page}</span>
                <span className="cameo-chevron" aria-hidden="true">⌄</span>
              </button>
              <div className="cameo-pill-actions">
                <button className={`cameo-icon-button ${pinned ? "is-active" : ""}`} type="button"
                  aria-label={pinned ? "Unpin dock" : "Pin dock"} aria-pressed={pinned}
                  title={pinned ? "Always on top" : "Not pinned"} onClick={togglePinned}>
                  <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 4h8l-1 5 3 3v2H6v-2l3-3-1-5ZM12 14v6" /></svg>
                </button>
                <button className="cameo-icon-button" type="button" aria-label="Snap to top center"
                  title="Snap to top center" onClick={() => onSnapToTop?.()}>
                  <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 5h16M12 5v14m-5-5 5 5 5-5" /></svg>
                </button>
                <button className="cameo-icon-button" type="button" aria-label="Minimize to orb"
                  title="Minimize" onClick={() => changeMode("orb")}>
                  <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 12h14" /></svg>
                </button>
                <button className="cameo-icon-button cameo-close" type="button" aria-label="Collapse dock"
                  title="Collapse dock" onClick={() => changeMode(mode === "workspace" ? "dock" : "orb")}>
                  <svg viewBox="0 0 24 24" aria-hidden="true"><path d={mode === "workspace" ? "M5 12h14" : "M7 10l5 5 5-5"} /></svg>
                </button>
              </div>
            </>
          )}
          {mode === "orb" && (
            <button className="cameo-orb-expand" type="button" onClick={() => changeMode("dock")}
              aria-label="Expand dock" title="Expand dock"><span /></button>
          )}
        </header>

        <div className={`cameo-panel ${mode === "orb" ? "panel-hidden" : ""}`} aria-hidden={mode === "orb"}>
          <nav className="cameo-nav" aria-label="CAMEO sections">
            {NAV_GROUPS.map(group => (
              <div className="cameo-nav-group" key={group.title}>
                <span className="cameo-nav-group-title" aria-hidden="true">{group.title}</span>
                {group.items.map(label => {
                  const item = pages.find(p => p.label === label)!;
                  return (
                    <button key={item.label} className={`cameo-nav-item ${page === item.label ? "selected" : ""}`}
                      type="button" title={item.hint} aria-current={page === item.label ? "page" : undefined}
                      onClick={() => navigate(item.label)}>
                      <span className="cameo-nav-glyph" aria-hidden="true">{item.glyph}</span>
                      <span>{item.label}</span>
                    </button>
                  );
                })}
              </div>
            ))}
          </nav>

          <div className="cameo-content">
            {page === "Assistant" ? (
                <div className="cameo-chat-demo">
                  <div className="cameo-content-heading">
                    <div><span className="cameo-eyebrow">PERSONAL AI WORKSPACE</span><h2>What can I help you build?</h2></div>
                    <span className="cameo-status"><i /> Ready</span>
                  </div>
                  <div className="cameo-welcome">
                    <span className="cameo-welcome-mark"><img src="/logo.svg" alt="" /></span>
                    <div><strong>Your workspace, connected.</strong><p>Ask a question, plan a project, or continue a task. Your configured model and tools remain in control.</p></div>
                  </div>
                  {children}
                  <div className="cameo-quick-actions">
                    <button type="button" onClick={() => navigate("Tasks")}><span>▤</span><b>Build a project</b><small>Plan and track work</small></button>
                    <button type="button" onClick={() => navigate("Agents")}><span>◈</span><b>Choose an agent</b><small>Find a specialist</small></button>
                    <button type="button" onClick={() => navigate("Settings")}><span>⚙</span><b>Configure AI</b><small>Providers and models</small></button>
                  </div>
                  <form className="cameo-prompt" onSubmit={submitPrompt}>
                    <label className="sr-only" htmlFor="cameo-prompt-input">Message CAMEO</label>
                    <textarea id="cameo-prompt-input" value={prompt} onChange={e => setPrompt(e.target.value)}
                      placeholder="Message CAMEO…" rows={1} onKeyDown={e => {
                        if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); e.currentTarget.form?.requestSubmit(); }
                      }} />
                    <div className="cameo-prompt-footer"><span>Enter to send · Shift + Enter for a new line</span>
                      <button type="submit" disabled={!prompt.trim()} aria-label="Submit message">↑</button></div>
                  </form>
                  {!onSubmitPrompt && <p className="cameo-ui-note">UI preview · connect this composer to the existing chat service</p>}
                </div>
              ) : (
                <div className="cameo-placeholder-page">
                  <span className="cameo-placeholder-icon">{pages.find(p => p.label === page)?.glyph}</span>
                  <span className="cameo-eyebrow">CAMEO WORKSPACE</span>
                  <h2>{page}</h2>
                  <p>This redesigned shell is ready to host your existing {page.toLowerCase()} page.</p>
                  <button type="button" onClick={() => navigate("Assistant")}>Return to Assistant <span>→</span></button>
                </div>
              )
            }
          </div>
        </div>
      </section>
      <div className="cameo-stage-hint" aria-hidden="true">
        <span>●</span> CAMEO AI <i /> TOP-CENTER DOCK
      </div>
      {menuOpen && (
        <div className="cameo-menu-backdrop" onClick={() => setMenuOpen(false)}>
          <div className="cameo-page-menu" role="menu" aria-label="Choose a section" onClick={e => e.stopPropagation()}>
            <span className="cameo-eyebrow">NAVIGATE TO</span>
            {pages.map(item => <button key={item.label} role="menuitem" type="button" onClick={() => navigate(item.label)}>
              <span>{item.glyph}</span>{item.label}{page === item.label && <b>✓</b>}
            </button>)}
          </div>
        </div>
      )}
    </main>
  );
}
