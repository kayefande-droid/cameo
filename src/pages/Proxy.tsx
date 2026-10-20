import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface ProxyProfile {
  id: string;
  name: string;
  kind: "http" | "https" | "socks5";
  host: string;
  port: number;
  username: string | null;
  has_password: number;
  enabled: number;
}

interface TestResult {
  ok: boolean;
  message: string;
  via_proxy: boolean;
  elapsed_ms?: number;
  egress_ip?: string;
}

const EMPTY = {
  id: "",
  name: "",
  kind: "http",
  host: "",
  port: 8080,
  username: "",
  password: "",
  enabled: 0,
};

export default function Proxy() {
  const [profiles, setProfiles] = useState<ProxyProfile[]>([]);
  const [draft, setDraft] = useState<typeof EMPTY>({ ...EMPTY });
  const [test, setTest] = useState<TestResult | null>(null);
  const [saving, setSaving] = useState(false);

  const refresh = async () => {
    try {
      setProfiles(await invoke<ProxyProfile[]>("proxy_list"));
    } catch {
      setProfiles([]);
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const save = async () => {
    setSaving(true);
    try {
      const id = draft.id || `proxy-${Date.now()}`;
      await invoke("proxy_save", {
        profile: {
          id,
          name: draft.name || `${draft.kind}://${draft.host}:${draft.port}`,
          kind: draft.kind,
          host: draft.host,
          port: draft.port,
          username: draft.username || null,
          has_password: draft.password ? 1 : 0,
          enabled: draft.enabled,
        },
        password: draft.password || null,
      });
      setDraft({ ...EMPTY });
      await refresh();
    } catch (e) {
      setTest({ ok: false, message: String(e), via_proxy: false });
    } finally {
      setSaving(false);
    }
  };

  const delet = async (id: string) => {
    await invoke("proxy_delete", { id });
    await refresh();
  };

  const testProfile = async (p: ProxyProfile) => {
    setTest(null);
    // The Rust command is invoked through chat traffic for real proxy verification;
    // here we test the general egress (direct vs proxied) via the HTTP status endpoint.
    try {
      const res = await invoke<TestResult>("proxy_test", { profileId: p.id });
      setTest(res);
    } catch (e) {
      setTest({ ok: false, message: String(e), via_proxy: true });
    }
  };

  return (
    <>
      <div className="card">
        <h3>Saved profiles</h3>
        {profiles.length === 0 && <p className="note">No proxy profiles yet. Passwords are stored in Windows Credential Manager — never in the database.</p>}
        {profiles.map((p) => (
          <div key={p.id} style={{ display: "flex", alignItems: "center", gap: 10, padding: "8px 0", borderBottom: "1px solid var(--border)" }}>
            <span className="pill accent">{p.kind}</span>
            <span>
              <strong style={{ fontSize: 13 }}>{p.name}</strong>
              <div className="note mono">
                {p.host}:{p.port}
                {p.username ? ` (user ${p.username})` : ""}
                {p.has_password ? " · password stored in secure store" : ""}
              </div>
            </span>
            <span style={{ marginLeft: "auto", display: "flex", gap: 6 }}>
              <span className={`pill ${p.enabled ? "success" : "muted"}`}>{p.enabled ? "enabled" : "disabled"}</span>
              <button className="btn small" onClick={() => testProfile(p)}>Test</button>
              <button className="btn small danger" onClick={() => delet(p.id)}>Delete</button>
            </span>
          </div>
        ))}
        {test && (
          <div className="note" style={{ color: test.ok ? "var(--success)" : "var(--danger)", marginTop: 10 }}>
            {test.ok ? "✔" : "✖"} {test.message}
            {test.elapsed_ms ? ` (${test.elapsed_ms}ms)` : ""}
            {test.egress_ip && test.ok ? ` · egress IP ${test.egress_ip}` : ""}
          </div>
        )}
        <p className="note" style={{ marginTop: 12 }}>
          Scope disclosure: the proxy is applied to the app&apos;s AI provider traffic. It does not,
          by itself, change your Windows system proxy settings.
        </p>
      </div>

      <div className="card">
        <h3>Add / edit profile</h3>
        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 10, marginTop: 10 }}>
          <label className="field">Scheme
            <select value={draft.kind} onChange={(e) => setDraft({ ...draft, kind: e.target.value as typeof draft.kind })}>
              <option value="http">HTTP</option>
              <option value="https">HTTPS</option>
              <option value="socks5">SOCKS5</option>
            </select>
          </label>
          <label className="field">Name
            <input value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} placeholder="My proxy" />
          </label>
          <label className="field">Host
            <input value={draft.host} onChange={(e) => setDraft({ ...draft, host: e.target.value })} placeholder="proxy.example.com" className="mono" />
          </label>
          <label className="field">Port
            <input type="number" min={1} max={65535} value={draft.port} onChange={(e) => setDraft({ ...draft, port: parseInt(e.target.value || "8080", 10) })} className="mono" />
          </label>
          <label className="field">Username (optional)
            <input value={draft.username} onChange={(e) => setDraft({ ...draft, username: e.target.value })} className="mono" />
          </label>
          <label className="field">Password (stored securely)
            <input type="password" value={draft.password} onChange={(e) => setDraft({ ...draft, password: e.target.value })} placeholder="••••••" />
          </label>
        </div>
        <div style={{ display: "flex", gap: 8, marginTop: 12, alignItems: "center" }}>
          <label className="field" style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>
            <input
              type="checkbox"
              checked={draft.enabled === 1}
              style={{ width: "auto" }}
              onChange={(e) => setDraft({ ...draft, enabled: e.target.checked ? 1 : 0 })}
            />
            Enable this profile for AI traffic
          </label>
          <button className="btn primary" onClick={save} disabled={saving || !draft.host}>
            {saving ? "Saving…" : "Save profile"}
          </button>
          <button className="btn ghost" onClick={() => setDraft({ ...EMPTY })}>Clear</button>
        </div>
      </div>
    </>
  );
}
