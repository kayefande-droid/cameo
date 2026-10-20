import { useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import DeleteConfirm from "../components/DeleteConfirmPanel";
import TaskExecutionWindow from "../components/TaskExecutionWindow";
import { AGENTS, type TaskRecord, statusLabel } from "../types/tasks";
import type { ModelSelection } from "../App";
import { getPref, listTasks, setPref, upsertTask, addTaskEvent } from "../services/store";
import { hasKey, saveKey, deleteKey } from "../services/ai";
import { PROVIDERS, type AIProviderId } from "../types/ai";

export default function Tasks() {
  const [tasks, setTasks] = useState<TaskRecord[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const NEW = useMemo(() => `tasks-new-${Date.now()}`, []);

  useEffect(() => {
    void refresh();
    const interval = window.setInterval(() => void refresh(), 2000);
    return () => window.clearInterval(interval);
  }, [NEW]);

  const refresh = async (focusId?: string) => {
    const next = await listTasks();
    setTasks(next);
    if (focusId) setSelectedId(focusId);
  };

  const selected = tasks.find((t) => t.id === selectedId) ?? null;

  return (
    <>
      <div className="grid-2" style={{ flex: "0 0 auto" }}>
        <NewTaskPanel onCreated={(id) => void refresh(id)} />
      </div>
      <hr />
      <div style={{ display: "flex", gap: 16, flex: 1, minHeight: 0 }}>
        <div style={{ width: 340, display: "flex", flexDirection: "column", gap: 8, minWidth: 0 }}>
          <span className="section-title">Task history ({tasks.length})</span>
          <div style={{ overflowY: "auto", flex: 1, minHeight: 0 }}>
            {tasks.map((t) => (
              <div
                key={t.id}
                className={`card${selectedId === t.id ? " selected" : ""}`}
                style={{
                  padding: 10,
                  marginBottom: 8,
                  cursor: "pointer",
                  borderColor: selectedId === t.id ? "var(--accent-secondary)" : undefined,
                }}
                onClick={() => setSelectedId(t.id)}
              >
                <strong style={{ fontSize: 13 }}>{t.title || t.id}</strong>
                <div style={{ fontSize: 11, color: "var(--text-faint)" }}>
                  {AGENTS.find((a) => a.id === t.agent)?.codename ?? t.agent} · {statusLabel(t.status)}
                </div>
              </div>
            ))}
            {tasks.length === 0 && (
              <div style={{ color: "var(--text-faint)", padding: 16, fontSize: 13 }}>
                No tasks yet — create one above. Nothing is simulated.
              </div>
            )}
          </div>
        </div>
        <div style={{ flex: 1, minWidth: 0 }}>
          {selected ? (
            <>
              <TaskExecutionWindow task={selected} onRefresh={refresh} />
              <DeleteConfirm taskId={selected.id} onDeleted={refresh} />
            </>
          ) : (
            <div style={{ color: "var(--text-faint)", padding: 24 }}>
              Select a task from the history to view its execution window.
            </div>
          )}
        </div>
      </div>
    </>
  );
}

function NewTaskPanel({ onCreated }: { onCreated: (taskId: string) => void }) {
  const [title, setTitle] = useState("");
  const [desc, setDesc] = useState("");
  const [agent, setAgent] = useState("forge");
  const [projectRoot, setProjectRoot] = useState("");
  const [modelSel, setModelSel] = useState<ModelSelection | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [provider, setProvider] = useState<AIProviderId>("openrouter");
  const [model, setModel] = useState("");
  const [keyState, setKeyState] = useState<"unknown" | "present" | "missing">("unknown");
  const [modelLoaded, setModelLoaded] = useState(false);

  useEffect(() => {
    setModelLoaded(false);
    void getPref("model_selection").then((s) => {
      if (s) {
        try {
          const sel = JSON.parse(s) as ModelSelection;
          setModelSel(sel);
          setProvider(sel.provider);
          setModel(sel.model);
        } catch { /* ignore */ }
      }
    }).finally(() => setModelLoaded(true));
    void hasKey(provider).then((has) => setKeyState(has ? "present" : "missing"));
  }, [provider]);

  const pickModel = (p: AIProviderId, m: string) => {
    setProvider(p);
    setModel(m);
    const sel = { provider: p, model: m };
    setModelSel(sel);
    void setPref("model_selection", JSON.stringify(sel));
  };

  const createAndPlan = async () => {
    if (!title.trim()) return;
    setBusy(true);
    setStatus("Saving task for specialist planning…");
    try {
      if (!modelSel?.model?.trim()) throw new Error("Choose a provider/model in Settings before running agents.");
      const taskId = `task-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      const task: TaskRecord = {
        id: taskId,
        title: title.trim(),
        description: desc.trim(),
        agent: agent as TaskRecord["agent"],
        status: "queued",
        currentStep: "Ready for specialist planning",
        steps: [],
        changedFiles: [],
        logs: [],
        errors: [],
        projectRoot: projectRoot || undefined,
        createdAt: Date.now(),
        updatedAt: Date.now(),
      };
      await upsertTask(task);
      await addTaskEvent(task.id, "queued", `Queued for ${AGENTS.find((a) => a.id === agent)?.codename ?? agent}`);
      setTitle("");
      setDesc("");
      setStatus("Task queued. The selected specialist can now create its real plan in the task window.");
      onCreated(taskId);
    } catch (e) {
      setStatus(String(e));
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    const key = prompt(`Enter the API key for ${PROVIDERS.find((p) => p.id === provider)!.label}:`) ?? "";
    if (!key.trim()) return;
    await saveKey(provider, key);
    setKeyState("present");
  };

  const remove = async () => {
    await deleteKey(provider);
    setKeyState("missing");
  };

  return (
    <div className="card">
      <h3>
        <span className="section-title" style={{ display: "flex", justifyContent: "space-between", flex: 1 }}>
          <span>New task</span>
          <span style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <select
              value={provider}
              aria-label="Task provider"
              onChange={(e) => pickModel(e.target.value as AIProviderId, model)}
              style={{ width: 170 }}
            >
              {PROVIDERS.map((p) => (
                <option key={p.id} value={p.id}>{p.label}</option>
              ))}
            </select>
            <input
              value={model}
              placeholder="Model"
              aria-label="Task model"
              style={{ width: 220 }}
              onChange={(e) => pickModel(provider, e.target.value)}
            />
            <button className="btn small" onClick={save}>Set key</button>
            {keyState === "present" && (
              <button className="btn small danger" onClick={remove}>Delete key</button>
            )}
          </span>
        </span>
      </h3>
      <div style={{ display: "flex", gap: 8, marginTop: 10, alignItems: "flex-end" }}>
        <label className="field" style={{ flex: 2 }}>
          Title
          <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="What should the specialist do?" />
        </label>
        <label className="field" style={{ flex: 2 }}>
          Details (optional)
          <input value={desc} onChange={(e) => setDesc(e.target.value)} placeholder="Constraints, goals…" />
        </label>
        <label className="field" style={{ width: 160 }}>
          Specialist
          <select aria-label="Specialist agent" value={agent} onChange={(e) => setAgent(e.target.value)}>
            {AGENTS.map((a) => (
              <option key={a.id} value={a.id}>{a.codename}</option>
            ))}
          </select>
        </label>
        <button
          className="btn"
          onClick={async () => {
            const selected = await open({ directory: true, multiple: false });
            if (typeof selected === "string") setProjectRoot(selected);
          }}
        >
          {projectRoot ? "Project selected ✓" : "Select project folder"}
        </button>
        <button className="btn primary" onClick={createAndPlan} disabled={busy || !modelLoaded || !title.trim() || !modelSel?.model}>
          {busy ? "Saving…" : "Create task"}
        </button>
      </div>
      {projectRoot && <div className="note mono">Approved workspace boundary: {projectRoot}</div>}
      {status && <div className="note">{status}</div>}
    </div>
  );
}
