import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import ConfirmDialog from "../components/Dialogs";
import {
  addMemory,
  deleteMemory,
  listMemory,
  updateMemory,
  type MemoryEntry,
} from "../services/memory";
import {
  getCapabilityRegistry,
  listCapabilities,
  type CapabilityEntry,
  type CapabilityRegistry,
} from "../services/capabilities";

interface ToolStatus {
  id: string;
  name: string;
  available: boolean;
  version: string;
  detail: string;
}

interface ProjectInspection {
  path: string;
  kind: string;
  frameworks: string[];
  scripts: [string, string][];
  top_level: string[];
  key_files: [string, string][];
}

interface CommandOutput {
  exit_code: number;
  stdout: string;
  stderr: string;
}

const MEMORY_KINDS: { value: MemoryEntry["kind"]; label: string }[] = [
  { value: "decision", label: "Decision" },
  { value: "fact", label: "Fact" },
  { value: "lesson", label: "Lesson" },
  { value: "runbook", label: "Runbook" },
  { value: "preference", label: "Preference" },
  { value: "note", label: "Note" },
  { value: "unresolved-issue", label: "Unresolved issue" },
];

export default function Developer() {
  const [tools, setTools] = useState<ToolStatus[] | null>(null);
  const [scanning, setScanning] = useState(false);
  const [proj, setProj] = useState<ProjectInspection | null>(null);
  const [pickError, setPickError] = useState<string | null>(null);
  const [running, setRunning] = useState<string | null>(null);
  const [output, setOutput] = useState<CommandOutput | null>(null);
  const [confirmRun, setConfirmRun] = useState<string | null>(null);

  const [memory, setMemory] = useState<MemoryEntry[]>([]);
  const [memoryKind, setMemoryKind] = useState<MemoryEntry["kind"]>("decision");
  const [memoryTitle, setMemoryTitle] = useState("");
  const [memoryBody, setMemoryBody] = useState("");
  const [memoryBusy, setMemoryBusy] = useState(false);
  const [memoryError, setMemoryError] = useState<string | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editingTitle, setEditingTitle] = useState("");
  const [editingBody, setEditingBody] = useState("");
  const [registryError, setRegistryError] = useState<string | null>(null);
  const [registryEntries, setRegistryEntries] = useState<CapabilityEntry[] | null>(null);
  const [registrySource, setRegistrySource] = useState<CapabilityRegistry["source"] | null>(null);

  const reloadMemory = useCallback(async () => {
    try {
      setMemory(await listMemory());
      setMemoryError(null);
    } catch (e) {
      setMemoryError(String(e));
    }
  }, []);

  useEffect(() => {
    void reloadMemory();
    try {
      setRegistryEntries(listCapabilities());
      setRegistrySource(getCapabilityRegistry().source);
    } catch (e) {
      setRegistryEntries(null);
      setRegistryError(String(e));
    }
  }, [reloadMemory]);

  const scan = useCallback(async () => {
    setScanning(true);
    try {
      setTools(await invoke<ToolStatus[]>("detect_dev_tools"));
    } catch (e) {
      setTools([]);
      setPickError(String(e));
    } finally {
      setScanning(false);
    }
  }, []);

  useEffect(() => {
    void scan();
  }, [scan]);

  const pickProject = async () => {
    setPickError(null);
    try {
      // Lazy import so the dialog plugin failure is visible in the UI, not a crash.
      const { open } = await import("@tauri-apps/plugin-dialog");
      const dir = await open({ directory: true });
      if (!dir || typeof dir !== "string") return;
      const res = await invoke<ProjectInspection>("inspect_project", { dir });
      setProj(res);
      setOutput(null);
    } catch (e) {
      setPickError(`Project pick/inspect failed: ${e}`);
    }
  };

  const runCmd = async (command: string) => {
    if (!proj) return;
    setRunning(command);
    setOutput(null);
    try {
      const res = await invoke<CommandOutput>("run_project_command", {
        dir: proj.path,
        command,
      });
      setOutput(res);
    } catch (e) {
      setOutput({ exit_code: -1, stdout: "", stderr: String(e) });
    } finally {
      setRunning(null);
    }
  };

  const hasGit = tools?.some((t) => t.id === "git" && t.available) ?? false;

  const saveMemory = async () => {
    setMemoryBusy(true);
    setMemoryError(null);
    try {
      await addMemory({ kind: memoryKind, title: memoryTitle, body: memoryBody });
      setMemoryTitle("");
      setMemoryBody("");
      await reloadMemory();
    } catch (e) {
      setMemoryError(String(e));
    } finally {
      setMemoryBusy(false);
    }
  };

  const saveEdit = async (entry: MemoryEntry) => {
    setMemoryBusy(true);
    setMemoryError(null);
    try {
      await updateMemory(entry.id, { title: editingTitle, body: editingBody });
      setEditingId(null);
      await reloadMemory();
    } catch (e) {
      setMemoryError(String(e));
    } finally {
      setMemoryBusy(false);
    }
  };

  const toggleMemory = async (entry: MemoryEntry) => {
    setMemoryBusy(true);
    setMemoryError(null);
    try {
      await updateMemory(entry.id, { status: entry.status === "active" ? "superseded" : "active" });
      await reloadMemory();
    } catch (e) {
      setMemoryError(String(e));
    } finally {
      setMemoryBusy(false);
    }
  };

  const removeMemory = async (entry: MemoryEntry) => {
    setMemoryBusy(true);
    setMemoryError(null);
    try {
      await deleteMemory(entry.id);
      await reloadMemory();
    } catch (e) {
      setMemoryError(String(e));
    } finally {
      setMemoryBusy(false);
    }
  };

  const exportMemory = () => {
    const blob = new Blob([JSON.stringify({ schema: "cameo.memory.export.v1", entries: memory }, null, 2)], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = "cameo-project-memory.json";
    a.click();
    URL.revokeObjectURL(url);
  };

  return (
    <>
      <div className="card">
        <h3>
          <span className="section-title">PC build tools ({scanning ? "scanning with real version probes…" : `${tools?.filter((t) => t.available).length ?? 0}/${tools?.length ?? 0} available`})</span>
          <button className="btn small" onClick={scan} disabled={scanning}>
            {scanning ? "Scanning…" : "Rescan"}
          </button>
        </h3>
        <div className="mono" style={{ maxHeight: 220, overflowY: "auto", marginTop: 8 }}>
          {(tools ?? []).map((t) => (
            <div key={t.id}>
              <span style={{ color: t.available ? "var(--success)" : "var(--text-faint)" }}>
                {t.available ? "✔" : "·"}
              </span>{" "}
              <strong>{t.name}</strong> {t.version && `— ${t.version}`}
            </div>
          ))}
        </div>
      </div>

      <div className="card">
        <h3>
          <span className="section-title">Project workflow</span>
          <button className="btn small primary" onClick={pickProject}>Open project folder…</button>
        </h3>
        {!proj && <p className="note">Pick a folder to inspect its framework, scripts and key files — like ATLAS would.</p>}
        {proj && (
          <>
            <p className="note mono">{proj.kind} at {proj.path}</p>
            <p className="note">
              Frameworks: {proj.frameworks.join(", ") || "unknown"}
            </p>
            {proj.scripts.length > 0 && (
              <p className="note">Scripts: {proj.scripts.map(([k, v]) => `${k} → ${v}`).join(" · ")}</p>
            )}
            <p className="note">Top level: {proj.top_level.join(", ").slice(0, 600)}</p>
            <hr />
            <span className="section-title">Allowlisted build/test commands</span>
            <div style={{ display: "flex", gap: 8, marginTop: 8, flexWrap: "wrap" }}>
              {["npm test", "npm run test", "npm run typecheck", "npm run build", "cargo test", "cargo check", "python -m pytest", "go test"].map((c) => (
                <button key={c} className="btn small" onClick={() => setConfirmRun(c)} disabled={running != null}>
                  {c}
                </button>
              ))}
            </div>
            {output && (
              <div className="log-window" style={{ marginTop: 10, color: output.exit_code === 0 ? "#9FD6DB" : "var(--danger)" }}>
                {`exit code: ${output.exit_code}\n\n${output.stderr || ""}${output.stdout || ""}`}
              </div>
            )}
          </>
        )}
        {pickError && <div className="note danger">{pickError}</div>}
        {!hasGit && <p className="note">Tip: Git was not detected — version control safeguards in this workflow are limited.</p>}
      </div>

      <div className="card" aria-label="Capability registry">
        <h3>
          <span className="section-title">Capability registry</span>
          <span className="pill muted">{registryEntries?.length ?? 0} capabilities</span>
        </h3>
        {registryError && <div className="note danger">{registryError}</div>}
        {registrySource && (
          <p className="note" style={{ marginTop: 6 }}>
            Adapted for CAMEO from {registrySource.upstream} — {registrySource.upstreamVersion} ({registrySource.upstreamLicense}). {registrySource.note}
          </p>
        )}
        <div style={{ maxHeight: 260, overflowY: "auto", marginTop: 8 }}>
          {(registryEntries ?? []).map((entry) => (
            <div key={entry.id} style={{ padding: "8px 4px", borderBottom: "1px solid var(--border)" }}>
              <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                <strong className="mono">{entry.id}</strong>
                <span className="pill muted">{entry.agent.toUpperCase()} · {entry.phase}</span>
                <span className={`pill ${entry.availability === "active" ? "success" : "muted"}`}>{entry.availability}</span>
              </div>
              <p className="note" style={{ marginTop: 4 }}>{entry.description}</p>
              <p className="note mono" style={{ fontSize: 11 }}>tools: {entry.tools.join(", ")} · permissions: {entry.permissions.join(", ") || "none"}{entry.dependencies.length > 0 && ` · requires: ${entry.dependencies.join(", ")}`}</p>
            </div>
          ))}
        </div>
      </div>

      <div className="card" aria-label="Project memory">
        <h3>
          <span className="section-title">Project memory ({memory.filter((entry) => entry.status === "active").length} active / {memory.length} total)</span>
          <span style={{ display: "flex", gap: 8 }}>
            <button className="btn small" onClick={() => void reloadMemory()} disabled={memoryBusy}>Refresh</button>
            <button className="btn small" onClick={exportMemory} disabled={memory.length === 0}>Export JSON</button>
          </span>
        </h3>
        <p className="note" style={{ marginTop: 6 }}>
          Approved project knowledge (decisions, verified fixes, commands, patterns). Stored locally in the CAMEO database only — never sent to a model provider. Credential-like values are redacted before saving; do not store secrets here.
        </p>
        <div className="grid-2" style={{ marginTop: 10 }}>
          <label className="field">
            Kind
            <select value={memoryKind} onChange={(e) => setMemoryKind(e.target.value as MemoryEntry["kind"])}>
              {MEMORY_KINDS.map((kind) => <option key={kind.value} value={kind.value}>{kind.label}</option>)}
            </select>
          </label>
          <label className="field">
            Title
            <input value={memoryTitle} maxLength={200} onChange={(e) => setMemoryTitle(e.target.value)} placeholder="Short summary" />
          </label>
        </div>
        <label className="field" style={{ marginTop: 8 }}>
          Body
          <textarea value={memoryBody} maxLength={20_000} rows={4} onChange={(e) => setMemoryBody(e.target.value)} placeholder="What should CAMEO remember about this project?" />
        </label>
        <button className="btn small primary" style={{ marginTop: 8 }} onClick={() => void saveMemory()} disabled={memoryBusy || !memoryTitle.trim() || !memoryBody.trim()}>
          {memoryBusy ? "Saving…" : "Save memory"}
        </button>
        {memoryError && <div className="note danger" style={{ marginTop: 8 }}>{memoryError}</div>}

        <div style={{ marginTop: 12 }}>
          {memory.length === 0 && <p className="note">No saved memory yet. Entries survive app restarts and are fully editable here.</p>}
          {memory.map((entry) => (
            <article key={entry.id} className="proposal-card" style={{ marginBottom: 10 }}>
              {editingId === entry.id ? (
                <>
                  <label className="field">Title
                    <input value={editingTitle} maxLength={200} onChange={(e) => setEditingTitle(e.target.value)} />
                  </label>
                  <label className="field">Body
                    <textarea value={editingBody} maxLength={20_000} rows={4} onChange={(e) => setEditingBody(e.target.value)} />
                  </label>
                  <div style={{ display: "flex", gap: 8 }}>
                    <button className="btn small primary" onClick={() => void saveEdit(entry)} disabled={memoryBusy || !editingTitle.trim() || !editingBody.trim()}>Save edit</button>
                    <button className="btn small" onClick={() => setEditingId(null)} disabled={memoryBusy}>Cancel</button>
                  </div>
                </>
              ) : (
                <>
                  <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                    <strong>{entry.title}</strong>
                    <span className="pill muted">{entry.kind}</span>
                    <span className={`pill ${entry.status === "active" ? "success" : "muted"}`}>{entry.status}</span>
                  </div>
                  <pre className="note" style={{ whiteSpace: "pre-wrap", marginTop: 6 }}>{entry.body}</pre>
                  <p className="note" style={{ fontSize: 11 }}>saved {new Date(entry.createdAt).toLocaleString()} · updated {new Date(entry.updatedAt).toLocaleString()}</p>
                  <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                    <button className="btn small" onClick={() => { setEditingId(entry.id); setEditingTitle(entry.title); setEditingBody(entry.body); }} disabled={memoryBusy}>Edit</button>
                    <button className="btn small" onClick={() => void toggleMemory(entry)} disabled={memoryBusy}>{entry.status === "active" ? "Disable" : "Enable"}</button>
                    <button className="btn small danger" onClick={() => void removeMemory(entry)} disabled={memoryBusy}>Delete</button>
                  </div>
                </>
              )}
            </article>
          ))}
        </div>
      </div>

      {confirmRun && proj && (
        <ConfirmDialog
          spec={{
            title: `Run ${confirmRun}?`,
            body: `This runs ${confirmRun} inside:\n${proj.path}`,
            confirmLabel: "Run",
            onConfirm: () => {
              const c = confirmRun;
              setConfirmRun(null);
              void runCmd(c);
            },
            onCancel: () => setConfirmRun(null),
          }}
        />
      )}
    </>
  );
}
