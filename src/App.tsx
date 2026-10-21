import { useCallback, useEffect, useRef, useState } from "react";
import { listTasks } from "./services/store";
import { AGENTS, type AgentId, type TaskRecord } from "./types/tasks";
import type { AgentActivity } from "./components/AgentAvatar";
import Sidebar, { NAV_ITEMS, type PageId } from "./components/Sidebar";
import { getPref, setPref } from "./services/store";
import { invoke } from "@tauri-apps/api/core";
import type { AIProviderId } from "./types/ai";
import Chat from "./pages/Chat";
import Tasks from "./pages/Tasks";
import Agents from "./pages/Agents";
import Connections from "./pages/Connections";
import Developer from "./pages/Developer";
import Proxy from "./pages/Proxy";
import Browser from "./pages/Browser";
import Camera from "./pages/Camera";
import Voice from "./pages/Voice";
import Power from "./pages/Power";
import Settings from "./pages/Settings";
import About from "./pages/About";
import AgentAvatar from "./components/AgentAvatar";

export interface ModelSelection {
  provider: AIProviderId;
  model: string;
}

const TITLES: Record<PageId, string> = Object.fromEntries(
  NAV_ITEMS.map((n) => [n.id, n.label])
) as Record<PageId, string>;

type WindowMode = "orb" | "dock" | "panel" | "full";

export default function App() {
  const [page, setPage] = useState<PageId>("home");
  const [collapsed, setCollapsed] = useState(false);
  const [mode, setMode] = useState<WindowMode>("dock");
  const compact = mode === "dock" || mode === "panel" || mode === "orb";
  const [dockExpanded, setDockExpanded] = useState(false);
  const [activeRequestId, setActiveRequestId] = useState<string | null>(null);
  const activeRequestRef = useRef<string | null>(null);
  const setActiveRequest = useCallback((requestId: string | null) => {
    activeRequestRef.current = requestId;
    setActiveRequestId(requestId);
  }, []);
  const [tasks, setTasks] = useState<TaskRecord[]>([]);
  const [audioLevel, setAudioLevel] = useState(0);
  const [listening, setListening] = useState(false);
  const dockTimer = useRef<number | null>(null);
  const collapseTimer = useRef<number | null>(null);
  const modeRef = useRef<WindowMode>("dock");
  const tasksByAgent = new Map<AgentId, TaskRecord>();
  for (const task of tasks) {
    if (!tasksByAgent.has(task.agent) && (task.status === "queued" || task.status === "planning" || task.status === "awaiting_approval" || task.status === "awaiting_verification" || task.status === "running" || task.status === "verifying")) {
      tasksByAgent.set(task.agent, task);
    }
  }
  for (const task of tasks) {
    if (!tasksByAgent.has(task.agent) && (task.status === "failed" || task.status === "completed" && Date.now() - task.updatedAt < 5000)) {
      tasksByAgent.set(task.agent, task);
    }
  }
  const [pinned, setPinned] = useState(true);
  const [selection, setSelection] = useState<ModelSelection | null>(null);
  const [selectionLoaded, setSelectionLoaded] = useState(false);

  useEffect(() => {
    (async () => {
      const pinPref = await getPref("dock_pinned");
      const wantMode: WindowMode = "dock";
      const wantPin = pinPref === null ? true : pinPref === "1";
      modeRef.current = wantMode;
      setMode(wantMode);
      setPinned(wantPin);
      document.body.classList.toggle("compact", true);
      document.body.classList.toggle("orb", false);
      await setPref("dock_mode", "dock").catch(() => undefined);
      await setPref("dock_compact", "1").catch(() => undefined);
      await invoke("set_window_mode", { mode: wantMode }).catch(() => undefined);
      await invoke("dock_reposition", { mode: wantMode }).catch(() => undefined);
      await (await import("@tauri-apps/api/window")).getCurrentWindow().show().catch(() => undefined);
      void listTasks().then(setTasks);
      const stored = await getPref("model_selection");
      if (stored) {
        try {
          const current = JSON.parse(stored) as ModelSelection;
          // Migrate prior default brains to the verified serving model.
          // A hand-picked model is never touched.
          if (
            (current.provider === "nvidia" && current.model === "deepseek-ai/deepseek-r1") ||
            (current.provider === "openrouter" && current.model === "deepseek/deepseek-r1") ||
            (current.provider === "openrouter" && current.model === "openai/gpt-5.5-pro")
          ) {
            const { DEFAULT_BRAIN_SELECTION } = await import("./services/ai");
            const brain: ModelSelection = { ...DEFAULT_BRAIN_SELECTION };
            setSelection(brain);
            await setPref("model_selection", JSON.stringify(brain)).catch(() => undefined);
          } else {
            setSelection(current);
          }
        } catch {
          /* ignore malformed preference */
        }
      } else {
        // First run: default the main brain to the verified serving model
        // (STEP-5-Preview on NVIDIA) so agents answer immediately.
        const { DEFAULT_BRAIN_SELECTION } = await import("./services/ai");
        const brain: ModelSelection = { ...DEFAULT_BRAIN_SELECTION };
        setSelection(brain);
        await setPref("model_selection", JSON.stringify(brain)).catch(() => undefined);
      }
      setSelectionLoaded(true);
    })();
    return undefined;
  }, []);

  const changeMode = useCallback(async (next: WindowMode) => {
    modeRef.current = next;
    setMode(next);
    document.body.classList.toggle("compact", next !== "full");
    document.body.classList.toggle("orb", next === "orb");
    if (next !== "full") {
      await setPref("dock_mode", next).catch(() => undefined);
      await setPref("dock_compact", "1").catch(() => undefined);
    } else {
      await setPref("dock_compact", "0").catch(() => undefined);
    }
    await invoke("set_window_mode", { mode: next }).catch(() => undefined);
    if (next !== "full") await invoke("dock_reposition", { mode: next }).catch(() => undefined);
  }, []);

  useEffect(() => {
    const update = () => { void listTasks().then(setTasks); };
    update();
    const interval = window.setInterval(update, 1500);
    return () => {
      window.clearInterval(interval);
      if (dockTimer.current !== null) window.clearTimeout(dockTimer.current);
      if (collapseTimer.current !== null) window.clearTimeout(collapseTimer.current);
    };
  }, []);

  const expandPanel = useCallback(() => {
    if (dockTimer.current !== null) window.clearTimeout(dockTimer.current);
    setDockExpanded(true);
    if (collapseTimer.current !== null) window.clearTimeout(collapseTimer.current);
    if (mode !== "full") void changeMode("panel");
  }, [changeMode, mode]);

  const collapsePanel = useCallback(() => {
    setDockExpanded(false);
    if (collapseTimer.current !== null) window.clearTimeout(collapseTimer.current);
    if (mode === "panel") {
      collapseTimer.current = window.setTimeout(() => {
        if (modeRef.current === "panel") void changeMode("dock");
      }, 250);
    }
  }, [changeMode, mode]);

  const scheduleCollapse = useCallback(() => {
    if (dockTimer.current !== null) window.clearTimeout(dockTimer.current);
    dockTimer.current = window.setTimeout(() => {
      if (!activeRequestRef.current && !document.activeElement?.closest(".cameo-assistant-panel")) collapsePanel();
    }, 900);
  }, [collapsePanel]);

  useEffect(() => {
    let unlistenScale: (() => void) | undefined;
    let unlistenFocus: (() => void) | undefined;
    let alive = true;
    void import("@tauri-apps/api/window").then(async ({ getCurrentWindow }) => {
      const current = getCurrentWindow();
      const stopScale = await current.onScaleChanged(() => {
        if (modeRef.current !== "full") void invoke("dock_reposition", { mode: modeRef.current });
      });
      if (!alive) { stopScale(); return; }
      unlistenScale = stopScale;
      const stopFocus = await current.onFocusChanged(({ payload: focused }) => {
        if (focused || modeRef.current !== "panel" || activeRequestRef.current) return;
        if (dockTimer.current !== null) window.clearTimeout(dockTimer.current);
        dockTimer.current = window.setTimeout(() => {
          if (alive && modeRef.current === "panel") collapsePanel();
        }, 180);
      });
      if (!alive) { stopFocus(); return; }
      unlistenFocus = stopFocus;
    });
    const onResize = () => {
      if (modeRef.current !== "full") void invoke("dock_reposition", { mode: modeRef.current });
    };
    const onEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && modeRef.current === "panel") collapsePanel();
    };
    window.addEventListener("resize", onResize);
    window.addEventListener("keydown", onEscape);
    return () => {
      alive = false;
      unlistenScale?.();
      unlistenFocus?.();
      window.removeEventListener("resize", onResize);
      window.removeEventListener("keydown", onEscape);
    };
  }, [collapsePanel]);

  const togglePin = useCallback(async () => {
    const next = !pinned;
    setPinned(next);
    await setPref("dock_pinned", next ? "1" : "0").catch(() => undefined);
    const win = (await import("@tauri-apps/api/window")).getCurrentWindow();
    await win.setAlwaysOnTop(next).catch(() => undefined);
  }, [pinned]);

  const changeSelection = useCallback((sel: ModelSelection) => {
    setSelection(sel);
    void setPref("model_selection", JSON.stringify(sel));
  }, []);

  const navigate = useCallback((id: PageId) => setPage(id), []);

  const activityFor = (agent: AgentId): AgentActivity => {
    const task = tasksByAgent.get(agent);
    if (!task) return "idle";
    if (task.errors.length > 0 || task.status === "failed") return "error";
    if (task.status === "awaiting_approval" || task.status === "awaiting_verification") return "approval";
    if (task.status === "queued" || task.status === "planning") return "thinking";
    if (task.status === "running" || task.status === "verifying") return "working";
    if (task.status === "completed") return "completed";
    return "idle";
  };

  return (
    <div className={`app-shell ${mode}`} data-mode={mode}>
      {mode === "full" && (
        <Sidebar
          page={page}
          onNavigate={navigate}
          collapsed={collapsed}
          onToggle={() => setCollapsed((c) => !c)}
        />
      )}
      <main className="main-area" aria-busy={!selectionLoaded}>
        {mode === "orb" ? (
          <button className="orb-launcher" onClick={() => void changeMode("dock")} aria-label="Expand CAMEO floating dock">
            <AgentAvatar agent="cameo" size={50} activity={listening ? "listening" : activeRequestId ? "working" : activityFor("cameo")} audioLevel={audioLevel} />
            <span className="sr-only">Expand CAMEO dock</span>
          </button>
        ) : <>
        <header
          className="main-header dock-drag"
          onMouseDown={(event) => {
            if (event.button === 0 && (event.target === event.currentTarget || (event.target as HTMLElement).closest(".drag-handle"))) {
              void import("@tauri-apps/api/window").then(({ getCurrentWindow }) => getCurrentWindow().startDragging());
            }
          }}
          data-tauri-drag-region
        >
          {compact ? (
            <>
              <button className="dock-brand" onClick={dockExpanded ? collapsePanel : expandPanel} aria-expanded={dockExpanded} aria-label={dockExpanded ? "Collapse CAMEO assistant panel" : "Expand CAMEO assistant panel"}>
                <span className="dock-mark"><img src="/logo.svg" alt="" width={20} height={20} /></span>
                <span className="dock-wordmark">CAMEO</span>
              </button>
              <div className="dock-agent-row" aria-label="CAMEO agent activity">
                {AGENTS.map((agent) => {
                  const activity = agent.id === "cameo" ? listening ? "listening" : activeRequestId ? "working" : activityFor(agent.id) : activityFor(agent.id);
                  const task = tasksByAgent.get(agent.id);
                  return <button type="button" className="dock-agent-button" key={agent.id} title={`${agent.codename}${task ? ` · ${task.currentStep || task.status}` : " · idle"}`} onClick={() => { if (task) void navigate("tasks"); else expandPanel(); }}><AgentAvatar agent={agent.id} size={34} activity={activity} audioLevel={agent.id === "cameo" ? audioLevel : 0} /><span className={`dock-agent-state state-${activity}`} /></button>;
                })}
              </div>
              <button className="dock-prompt-trigger" onClick={expandPanel} aria-label="Open CAMEO assistant">{activeRequestId ? "Working…" : "Ask anything"}</button>
              <span className={`dock-live ${tasks.some((task) => task.status === "running" || task.status === "planning" || task.status === "verifying") || activeRequestId ? "is-active" : ""}`} aria-label={activeRequestId ? "Assistant is working" : "Assistant is idle"} />
            </>
          ) : <h1 className="header-title">{TITLES[page]}</h1>}
          {selection && (
            <span className="pill accent dock-chip" title="Active provider/model">
              <span className="dot" />
              {selection.provider}
              {compact ? "" : ` · ${selection.model || "no model chosen"}`}
            </span>
          )}
          <span className="header-actions">
            {compact && dockExpanded && <button className="dock-tool-button" title="Expand full CAMEO workspace" aria-label="Expand full CAMEO workspace" onClick={() => { setDockExpanded(false); void changeMode("full"); }}>↗</button>}
            {compact && <button className="dock-tool-button" onClick={() => { navigate("settings"); setDockExpanded(false); void changeMode("full"); }} aria-label="Open CAMEO settings" title="Settings">⚙</button>}
            <button className="dock-tool-button" onClick={togglePin} title={pinned ? "Unpin from top" : "Keep above other windows"} aria-label={pinned ? "Unpin window" : "Pin window to front"}>{pinned ? "⌖" : "◇"}</button>
            {!compact && <button className="btn small ghost" onClick={() => void changeMode("dock")} aria-label="Return to CAMEO floating dock">Dock</button>}
            {compact && !dockExpanded && <button className="dock-expand-button" onClick={expandPanel} aria-label="Open CAMEO assistant panel">⌄</button>}
            {compact && dockExpanded && <button className="dock-expand-button" onClick={collapsePanel} aria-label="Dismiss CAMEO assistant panel">×</button>}
          </span>
        </header>
        {compact ? (
          <div className={`cameo-assistant-panel${dockExpanded ? " is-expanded" : ""}`} aria-hidden={!dockExpanded} inert={!dockExpanded} onMouseEnter={() => { if (dockTimer.current !== null) window.clearTimeout(dockTimer.current); }} onMouseLeave={scheduleCollapse}>
            <>
              <div className="assistant-panel-heading">
                <div><span className="assistant-kicker">CAMEO · DESKTOP ASSISTANT</span><strong>{activeRequestId ? "Working on your request" : tasks.find((task) => task.status === "running" || task.status === "planning" || task.status === "verifying")?.currentStep || "Ready when you are"}</strong></div>
                <span className={`assistant-state ${activeRequestId ? "is-active" : ""}`}><i />{activeRequestId ? "Working" : "Ready"}</span>
              </div>
              <div className="assistant-agent-strip" aria-label="Specialist agents">
                {AGENTS.map((agent) => <div className="assistant-agent" key={agent.id}><AgentAvatar agent={agent.id} size={40} activity={agent.id === "cameo" ? listening ? "listening" : activeRequestId ? "working" : activityFor(agent.id) : activityFor(agent.id)} audioLevel={agent.id === "cameo" ? audioLevel : 0} /><span>{agent.codename}</span></div>)}
              </div>
              <div className="assistant-conversation"><Chat selection={selection} onSelectionChange={changeSelection} compact onRequestChange={setActiveRequest} onAudioLevel={setAudioLevel} onListeningChange={setListening} active={dockExpanded} /></div>
              <div className="assistant-task-activity" aria-live="polite">
                {tasks.filter((task) => task.status === "queued" || task.status === "planning" || task.status === "running" || task.status === "verifying" || task.status === "awaiting_approval" || task.status === "awaiting_verification").slice(0, 2).map((task) => <button type="button" className={`assistant-task task-${task.status}`} key={task.id} onClick={() => { navigate("tasks"); setDockExpanded(false); void changeMode("full"); }}><AgentAvatar agent={task.agent} size={28} activity={activityFor(task.agent)} /><span><strong>{AGENTS.find((agent) => agent.id === task.agent)?.codename} · {task.title}</strong><small>{task.currentStep || task.status}{task.steps.length > 0 ? ` · ${task.steps.filter((step) => step.status === "done").length}/${task.steps.length} steps` : ""}</small></span><em>{task.status.replace(/_/g, " ")}</em></button>)}
              </div>
              <div className="assistant-panel-footer"><span>Enter to send · Shift+Enter for a new line</span><button className="link-button" onClick={() => { navigate("tasks"); setDockExpanded(false); void changeMode("full"); }}>Task history</button></div>
            </>
          </div>
        ) : <div className="page-content">
          {page === "home" && <Chat selection={selection} onSelectionChange={changeSelection} />}
          {page === "tasks" && <Tasks />}
          {page === "agents" && <Agents />}
          {page === "connections" && <Connections />}
          {page === "developer" && <Developer />}
          {page === "proxy" && <Proxy />}
          {page === "browser" && <Browser />}
          {page === "camera" && <Camera />}
          {page === "voice" && <Voice />}
          {page === "power" && <Power />}
          {page === "settings" && <Settings onSelectionChange={changeSelection} />}
          {page === "about" && <About />}
        </div>}
        </>}
      </main>
    </div>
  );
}
