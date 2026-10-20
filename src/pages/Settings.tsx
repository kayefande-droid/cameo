import { useEffect, useState } from "react";
import { deleteKey, hasKey, listModels, saveKey, testConnection } from "../services/ai";
import { invoke } from "@tauri-apps/api/core";
import { getPref, setPref } from "../services/store";
import { PROVIDERS, type AIProviderId, type ModelInfo } from "../types/ai";
import type { ModelSelection } from "../App";
import ConfirmDialog from "../components/Dialogs";

export default function Settings({ onSelectionChange }: { onSelectionChange?: (selection: ModelSelection) => void }) {
  const [active, setActive] = useState<AIProviderId>("openrouter");
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [filter, setFilter] = useState("");
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [keyState, setKeyState] = useState<"unknown" | "present" | "missing">("unknown");
  const [testMsg, setTestMsg] = useState<string | null>(null);
  const [selection, setSelectionLocal] = useState<ModelSelection | null>(null);
  const [reviewSelection, setReviewSelection] = useState<ModelSelection | null>(null);
  const [reviewUsesPrimary, setReviewUsesPrimary] = useState(true);
  const [favorite, setFavorite] = useState<string[]>([]);
  const [confirmClear, setConfirmClear] = useState(false);
  const [dockX, setDockX] = useState(0);
  const [dockY, setDockY] = useState(0);

  useEffect(() => {
    void Promise.all([getPref("dock_offset_x"), getPref("dock_offset_y")]).then(([x, y]) => {
      setDockX(Math.max(-600, Math.min(600, Number(x ?? 0) || 0)));
      setDockY(Math.max(-500, Math.min(500, Number(y ?? 0) || 0)));
    });
    void getPref("model_selection").then((stored) => {
      if (!stored) return;
      try {
        const current = JSON.parse(stored) as ModelSelection;
        setSelectionLocal(current);
        setActive(current.provider);
        onSelectionChange?.(current);
      } catch {
        /* Ignore a malformed stored selection and keep settings usable. */
      }
    });
    void getPref("review_model_selection").then((stored) => {
      if (!stored || stored === "primary") return;
      try {
        const current = JSON.parse(stored) as ModelSelection;
        if (["openrouter", "nvidia", "ollama", "compatible"].includes(current.provider) && current.model.trim()) {
          setReviewSelection(current);
          setReviewUsesPrimary(false);
        }
      } catch {
        /* Keep the primary model as the safe review fallback. */
      }
    });
    void (async () => {
      const favRaw = await getPref("favorite_models");
      if (favRaw) {
        try {
          setFavorite(JSON.parse(favRaw) as string[]);
        } catch {
          /* ignore malformed preference */
        }
      }
    })();
  }, []);

  const loadModels = async () => {
    setLoading(true);
    setErr(null);
    try {
      const ms = await listModels(active);
      const q = filter.toLowerCase();
      setModels(q ? ms.filter((m) => m.id.toLowerCase().includes(q)) : ms);
    } catch (e) {
      setModels([]);
      setErr(String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void hasKey(active).then((has) => setKeyState(has ? "present" : "missing"));
  }, [active]);

  const setFav = (id: string) => {
    const next = favorite.includes(id)
      ? favorite.filter((f) => f !== id)
      : [...favorite, id].slice(-10);
    setFavorite(next);
    void setPref("favorite_models", JSON.stringify(next));
  };

  const applyTheme = (mode: "light" | "dark" | "system") => {
    if (mode === "system") {
      document.documentElement.removeAttribute("data-theme");
      localStorage.removeItem("cameo-theme");
    } else {
      document.documentElement.dataset.theme = mode;
      localStorage.setItem("cameo-theme", mode);
    }
    void setPref("theme", mode).catch(() => undefined);
  };

  return (
    <>
      <div className="card">
        <h3>Providers</h3>
        <div className="grid-2" style={{ marginTop: 10 }}>
          {PROVIDERS.map((p) => (
            <div
              key={p.id}
              style={{ padding: 12, cursor: "pointer", border: "1px solid", borderRadius: 10, borderColor: active === p.id ? "var(--accent-secondary)" : "var(--border)" }}
              onClick={() => {
                setActive(p.id);
                setModels([]);
              }}
            >
              <h3 style={{ fontSize: 13 }}>
                {p.label}{" "}
                {keyState === "present" && p.id === active && <span className="pill success">key stored</span>}
              </h3>
              <p className="note" style={{ marginTop: 6 }}>{p.description}</p>
              {p.requiresKey && (
                <div style={{ display: "flex", gap: 6, marginTop: 8 }} onClick={(e) => e.stopPropagation()}>
                  <KeyButton
                    provider={p.id}
                    onSaved={() => void hasKey(p.id).then((has) => setKeyState(has ? "present" : "missing"))}
                  />
                  <button
                    className="btn small"
                    onClick={() =>
                      void testConnection(p.id).then((r) => setTestMsg(`${r.ok ? "✔" : "✖"} ${r.message}`))
                    }
                  >
                    Test connection
                  </button>
                </div>
              )}
            </div>
          ))}
        </div>
        {testMsg && <div className="note" style={{ marginTop: 10 }}>{testMsg}</div>}
      </div>

      <div className="card">
        <h3>
          <span className="section-title">Models for {PROVIDERS.find((x) => x.id === active)?.label}</span>
          <span style={{ display: "flex", gap: 8 }}>
            <input
              value={filter}
              placeholder="Filter models…"
              onChange={(e) => setFilter(e.target.value)}
              style={{ width: 220 }}
            />
            <button className="btn small" onClick={loadModels} disabled={loading}>
              {loading ? "Loading…" : "Refresh"}
            </button>
          </span>
        </h3>
        {err && <div className="note danger" style={{ marginTop: 8 }}>{err}</div>}
        {favorite.length > 0 && (
          <p className="note" style={{ marginTop: 8 }}>Favorites: {favorite.join(", ")}</p>
        )}
        <div style={{ maxHeight: 340, overflowY: "auto", marginTop: 10 }}>
          {models.length === 0 && !loading && (
            <div className="note" style={{ padding: 10 }}>
              No models loaded yet — press Refresh to query {PROVIDERS.find((x) => x.id === active)?.label}.
            </div>
          )}
          {models.map((m) => (
            <div
              key={m.id}
              style={{ display: "flex", alignItems: "center", gap: 8, padding: "8px 4px", borderBottom: "1px solid var(--border)" }}
            >
              <span className="mono" style={{ flex: 1 }}>{m.id}</span>
              <span style={{ display: "flex", gap: 4 }}>
                {m.capabilities.map((c) => (
                  <span key={c} className="pill muted" style={{ fontSize: 10 }}>{c}</span>
                ))}
              </span>
              {m.pricing_in_per_m != null && (
                <span className="note mono">
                  ${m.pricing_in_per_m.toFixed(2)}/M in · ${m.pricing_out_per_m?.toFixed(2)}/M out
                </span>
              )}
              <button className="btn small" onClick={() => setFav(m.id)} title="Toggle favorite">
                {favorite.includes(m.id) ? "★" : "☆"}
              </button>
              <button
                className="btn small primary"
                onClick={() => {
                  const sel = { provider: m.provider as AIProviderId, model: m.id };
                  setSelectionLocal(sel);
                  onSelectionChange?.(sel);
                  void setPref("model_selection", JSON.stringify(sel));
                }}
              >
                Use
              </button>
              <button
                className="btn small"
                aria-label={`Use ${m.id} for specialist reviews`}
                onClick={() => {
                  const sel = { provider: m.provider as AIProviderId, model: m.id };
                  setReviewSelection(sel);
                  setReviewUsesPrimary(false);
                  void setPref("review_model_selection", JSON.stringify(sel));
                }}
              >
                Review
              </button>
            </div>
          ))}
        </div>
        {selection && (
          <div className="note" style={{ marginTop: 8 }}>
            Selected: {selection.provider} / {selection.model}
          </div>
        )}
        <div className="note" style={{ marginTop: 8 }}>
          Specialist review model: {reviewUsesPrimary ? `primary (${selection?.provider ?? "not configured"} / ${selection?.model ?? "no model"})` : `${reviewSelection?.provider} / ${reviewSelection?.model}`}
          {!reviewUsesPrimary && (
            <button className="btn small" style={{ marginLeft: 8 }} onClick={() => {
              setReviewUsesPrimary(true);
              setReviewSelection(null);
              void setPref("review_model_selection", "primary");
            }}>Use primary for reviews</button>
          )}
        </div>
        <p className="note" style={{ marginTop: 6 }}>Review requests send the task description and bounded project snapshot to the review model shown here. Choosing a hosted model sends data to that provider; Ollama stays on this device. Each review run records its provider/model so retry and recovery do not silently change destinations.</p>
      </div>

      <div className="card">
        <h3>Dock position</h3>
        <p className="note" style={{ marginTop: 6 }}>
          Adjust the dock relative to the webcam/top edge. Horizontal ranges ±600px; vertical ranges −500px (up) to +500px (down). Values are saved and shared by orb, dock, and full modes.
        </p>
        <div className="dock-offset-controls">
          <label className="field">
            Horizontal offset ({dockX}px)
            <input type="range" min={-600} max={600} step={4} value={dockX} onChange={(e) => {
              const next = Number(e.target.value); setDockX(next); void setPref("dock_offset_x", String(next)).then(() => invoke("dock_reposition")).catch((error) => setErr(String(error)));
            }} />
          </label>
          <label className="field">
            Vertical offset ({dockY}px)
            <input type="range" min={-500} max={500} step={4} value={dockY} onChange={(e) => {
              const next = Number(e.target.value); setDockY(next); void setPref("dock_offset_y", String(next)).then(() => invoke("dock_reposition")).catch((error) => setErr(String(error)));
            }} />
          </label>
          <button className="btn small" onClick={() => {
            setDockX(0); setDockY(0);
            void Promise.all([setPref("dock_offset_x", "0"), setPref("dock_offset_y", "0")]).then(() => invoke("dock_reposition")).catch((error) => setErr(String(error)));
          }}>Reset to center below webcam</button>
        </div>
      </div>

      <div className="card">
        <h3>Appearance</h3>
        <div style={{ display: "flex", gap: 8, marginTop: 10 }}>
          {(["dark", "light", "system"] as const).map((t) => (
            <button key={t} className="btn small" onClick={() => applyTheme(t)}>
              {t}
            </button>
          ))}
        </div>
      </div>

      <div className="card">
        <h3>Stored data</h3>
        <p className="note" style={{ marginTop: 6 }}>
          API keys and proxy passwords live in Windows Credential Manager — never in the SQLite
          database or logs. Delete the active provider&apos;s key below; tasks, events, bookmarks and
          preferences are reviewable in their own pages.
        </p>
        <button className="btn small danger" style={{ marginTop: 8 }} onClick={() => setConfirmClear(true)}>
          Delete provider key for {active}
        </button>
      </div>

      {confirmClear && (
        <ConfirmDialog
          spec={{
            title: `Delete ${active} API key?`,
            body: "The key is removed from Windows Credential Manager. Requests with this provider will fail until a new key is saved.",
            confirmLabel: "Delete key",
            danger: true,
            onConfirm: async () => {
              await deleteKey(active);
              setKeyState("missing");
              setConfirmClear(false);
            },
            onCancel: () => setConfirmClear(false),
          }}
        />
      )}
    </>
  );
}

function KeyButton({
  provider,
  onSaved,
}: {
  provider: AIProviderId;
  onSaved: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [val, setVal] = useState("");
  const [err, setErr] = useState<string | null>(null);
  if (!open) {
    return (
      <button className="btn small" onClick={() => setOpen(true)}>
        Set API key
      </button>
    );
  }
  return (
    <span style={{ display: "flex", gap: 6 }}>
      <input
        type="password"
        placeholder="sk-…"
        value={val}
        onChange={(e) => setVal(e.target.value)}
        style={{ width: 180 }}
      />
      <button
        className="btn small primary"
        onClick={() => {
          void saveKey(provider, val)
            .then(() => {
              setOpen(false);
              setErr(null);
              setVal("");
              onSaved();
            })
            .catch((x) => setErr(String(x)));
        }}
      >
        Save
      </button>
      {err && <span className="note danger">{err}</span>}
    </span>
  );
}
