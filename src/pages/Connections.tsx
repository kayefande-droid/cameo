import { useCallback, useEffect, useState } from "react";
import {
  gwAuditList,
  gwConnectApiKey,
  gwDisconnect,
  gwListAll,
  gwOauthConnect,
  gwRegistry,
  gwSetSecret,
  gwSetPermissions,
  gwStopControl,
  gwStopStatus,
  gwTestConnection,
  mcpAdd,
  mcpInspectTools,
  mcpList,
  mcpRemove,
  mcpSetEnabled,
  mcpUpdate,
  mcpValidate,
} from "../services/gateway";
import type {
  AuditEvent,
  ConnectionStatus,
  McpServer,
  Permissions,
  ProviderRegistryEntry,
} from "../types/gateway";

const EMPTY_PERMS: Permissions = { read: true, prepare: false, write: false, sensitive: false };

const STATUS_LABEL: Record<ConnectionStatus | string, string> = {
  not_configured: "Not configured",
  disconnected: "Disconnected",
  connecting: "Connecting…",
  awaiting_user_authorization: "Awaiting your authorization",
  connected: "Connected & validated",
  connected_limited: "Connected · permission-limited",
  expired: "Expired or revoked",
  failed: "Failed",
};

const LEVELS: Array<{ key: keyof Permissions; label: string; hint: string }> = [
  { key: "read", label: "Read", hint: "Read authorized resources" },
  { key: "prepare", label: "Prepare", hint: "Draft messages, patches and previews" },
  { key: "write", label: "Write", hint: "Modify authorized files or remote resources" },
  { key: "sensitive", label: "Sensitive", hint: "Send email, push code, deploy — always requires per-action approval" },
];

export default function Connections() {
  const [registry, setRegistry] = useState<ProviderRegistryEntry[]>([]);
  const [rows, setRows] = useState<Record<string, Partial<Permissions>>>({});
  const [statuses, setStatuses] = useState<Record<string, string>>({});
  const [stopped, setStopped] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [messages, setMessages] = useState<Record<string, string>>({});
  const [renderKey, setRenderKey] = useState("");
  const [audit, setAudit] = useState<AuditEvent[]>([]);
  const [servers, setServers] = useState<McpServer[]>([]);
  const [draft, setDraft] = useState({
    name: "",
    transport: "stdio" as "stdio" | "http",
    command: "",
    argsText: "",
    cwd: "",
    url: "",
    envText: "",
    callLevel: "read",
  });
  const [inspect, setInspect] = useState<Record<string, string>>({});

  const setMessage = useCallback((key: string, text: string) => {
    setMessages((m) => ({ ...m, [key]: text }));
  }, []);

  const refreshAll = useCallback(async () => {
    try {
      const reg = await gwRegistry();
      setRegistry(reg);
      const list = await gwListAll();
      const permRows: Record<string, Partial<Permissions>> = {};
      const statusRows: Record<string, string> = {};
      for (const entry of reg) {
        const row = list.find((r) => r.provider === entry.provider);
        permRows[entry.provider] = row?.permissions && Object.keys(row.permissions).length > 0 ? row.permissions : EMPTY_PERMS;
        statusRows[entry.provider] = row?.status ?? "not_configured";
      }
      setRows(permRows);
      setStatuses(statusRows);
      const stop = await gwStopStatus();
      setStopped(Boolean(stop?.stop));
      try {
        setAudit((await gwAuditList(12)).slice(0, 12));
      } catch {
        /* audit is best-effort */
      }
      try {
        setServers(await mcpList());
      } catch {
        setServers([]);
      }
    } catch (e) {
      setMessage("_global", String(e));
    }
  }, [setMessage]);

  useEffect(() => {
    void refreshAll();
  }, [refreshAll]);

  const act = useCallback(
    async (key: string, fn: () => Promise<unknown>, okText?: string) => {
      setBusy(key);
      try {
        await fn();
        if (okText) setMessage(key, okText);
        await refreshAll();
      } catch (e) {
        setMessage(key, String(e));
      } finally {
        setBusy(null);
      }
    },
    [refreshAll, setMessage],
  );

  const connect = (provider: string) =>
    act(`connect_${provider}`, async () => {
      const entry = registry.find((r) => r.provider === provider);
      if (entry?.authMethod === "oauth") {
        await gwOauthConnect(provider as "google" | "github", ["read"]);
      }
    }, "Authorization opened in your browser — approve there, CAMEO waits on the loopback listener.");

  const test = (provider: string) =>
    act(`test_${provider}`, async () => {
      const res = await gwTestConnection(provider);
      setMessage(`test_${provider}`, ("message" in res && res.message) ? String(res.message) : JSON.stringify(res));
    });

  const disconnect = (provider: string) =>
    act(`disconnect_${provider}`, () => gwDisconnect(provider), "Disconnected — vault entries cleared. Follow the revoke guide to fully revoke provider-side.");

  const togglePerm = (provider: string, level: keyof Permissions, value: boolean) =>
    act(`perm_${provider}`, () => gwSetPermissions(provider, { ...(rows[provider] ?? EMPTY_PERMS), [level]: value }));

  const addMcp = () =>
    act("mcp_add", async () => {
      const args = draft.argsText.split(/\s+/).filter(Boolean);
      const env = draft.envText.split(/[,\n]/).map((s) => s.trim()).filter(Boolean);
      await mcpAdd({
        name: draft.name,
        transport: draft.transport,
        command: draft.transport === "stdio" ? draft.command : null,
        args: draft.transport === "stdio" ? args : null,
        cwd: draft.transport === "stdio" ? draft.cwd : null,
        url: draft.transport === "http" ? draft.url : null,
        env: env.length ? env : null,
        callLevel: draft.callLevel,
      });
      setDraft({ name: "", transport: "stdio", command: "", argsText: "", cwd: "", url: "", envText: "", callLevel: "read" });
    }, "Server saved (disabled). Review its details, set env values, then enable it.");

  const validateMcp = (id: string) =>
    act(`mcp_validate_${id}`, async () => {
      const res = await mcpValidate(id);
      if (res.authRequirement?.authRequired) {
        setInspect((s) => ({ ...s, [id]: res.authRequirement!.message }));
      } else if (res.validated) {
        setInspect((s) => ({ ...s, [id]: `Validated (protocol ${res.protocolVersion}) · ${res.tools?.length ?? 0} tools` }));
      } else {
        setInspect((s) => ({ ...s, [id]: res.error ?? "validation failed" }));
      }
    });

  const inspectMcp = (id: string) =>
    act(`mcp_inspect_${id}`, async () => {
      const res = await mcpInspectTools(id);
      const names = (res.tools ?? []).map((t) => `${t.name}${t.schemaOk === false ? " (malformed schema)" : ""}`).join(", ");
      setInspect((s) => ({ ...s, [id]: names ? `Tools: ${names}` : "No tools reported" }));
    });

  return (
    <div className="connections-page">
      <div className="card connections-stop-card">
        <div className="connections-stop-row">
          <strong>Global Stop</strong>
          <span>Immediately halts every agent-side connector and MCP tool call and kills stdio servers.</span>
          <button
            className="btn small"
            disabled={busy !== null}
            onClick={() => void act("stop", () => gwStopControl(!stopped)).then(() => setStopped((s) => !s))}
          >
            {stopped ? "Release Stop" : "Engage Stop"}
          </button>
        </div>
        {messages._global && <p className="connections-error">{messages._global}</p>}
      </div>
        {messages._global && <p className="connections-error">{messages._global}</p>}

      <h3 className="section-title">Providers</h3>
      {registry.map((entry) => {
        const status = (statuses[entry.provider] ?? "not_configured") as ConnectionStatus;
        return (
          <div className="card connection-card" key={entry.provider}>
            <div className="connection-head">
              <strong>{entry.display}</strong>
              <span className={`pill connection-status status-${status}`}>
                <span className="dot" />
                {STATUS_LABEL[status] ?? status}
              </span>
              <span className="pill">{entry.authMethod === "oauth" ? "OAuth + PKCE" : "API key"}</span>
            </div>
            <p className="connection-scopes">
              Requested scopes: {entry.requiredScopes.length > 0 ? entry.requiredScopes.join(", ") : "none (API key auth)"}
            </p>
            <div className="connection-tools">
              {entry.tools.map((tool) => (
                <span key={tool.name} className={`chip chip-${tool.level}`}>
                  {tool.name}
                  {tool.approvalNeeded ? " ⚠" : ""} · {tool.level}
                </span>
              ))}
            </div>
            <div className="connection-perms" role="group" aria-label={`${entry.display} permissions`}>
              {LEVELS.map((lv) => (
                <label key={lv.key} title={lv.hint}>
                  <input
                    type="checkbox"
                    checked={Boolean((rows[entry.provider] ?? EMPTY_PERMS)[lv.key])}
                    disabled={busy !== null}
                    onChange={(e) => void togglePerm(entry.provider, lv.key, e.target.checked)}
                  />
                  {lv.label}
                </label>
              ))}
            </div>
            <div className="connection-actions">
              {entry.authMethod === "oauth" && (
                <>
                  <button className="btn small" disabled={busy !== null} onClick={() => void connect(entry.provider)}>
                    Connect
                  </button>
                  <button className="btn small ghost" disabled={busy !== null} onClick={() => void gwSetSecret(entry.provider, "client_id", window.prompt(`${entry.display} OAuth client ID (from your Google/GitHub OAuth app):`) ?? "").then(() => undefined)}>
                    Save client ID
                  </button>
                </>
              )}
              {entry.authMethod === "api_key" && (
                <span className="connection-key-entry">
                  <input
                    type="password"
                    placeholder={`Paste ${entry.display} API key (stored in OS credential store only)`}
                    value={renderKey}
                    onChange={(e) => setRenderKey(e.target.value)}
                  />
                  <button
                    className="btn small"
                    disabled={busy !== null || !renderKey.trim()}
                    onClick={() => void act(`connect_${entry.provider}`, async () => {
                      await gwConnectApiKey(entry.provider as "render", renderKey.trim());
                      setRenderKey("");
                    }, "Key stored and validated with a read-only request.")}
                  >
                    Save & test
                  </button>
                </span>
              )}
              <button className="btn small ghost" disabled={busy !== null} onClick={() => void test(entry.provider)}>
                Test
              </button>
              <button className="btn small ghost" disabled={busy !== null} onClick={() => void disconnect(entry.provider)}>
                Disconnect
              </button>
            </div>
            {messages[`connect_${entry.provider}`] && <p className="connections-msg">{messages[`connect_${entry.provider}`]}</p>}
            {messages[`test_${entry.provider}`] && <p className="connections-msg">{messages[`test_${entry.provider}`]}</p>}
            {messages[`disconnect_${entry.provider}`] && <p className="connections-msg">{messages[`disconnect_${entry.provider}`]}</p>}
            {messages[`perm_${entry.provider}`] && <p className="connections-error">{messages[`perm_${entry.provider}`]}</p>}
            <details className="connection-guide">
              <summary>Revoke access guide</summary>
              <p>{entry.revokeGuide}</p>
            </details>
          </div>
        );
      })}

      <h3 className="section-title">MCP Server Manager</h3>
      {servers.map((srv) => (
        <div className="card connection-card" key={srv.id}>
          <div className="connection-head">
            <strong>{srv.name}</strong>
            <span className="pill">{srv.transport === "stdio" ? "stdio" : "HTTP"}</span>
            <span className={`pill connection-status status-${srv.status}`}>
              <span className="dot" />
              {srv.enabled ? srv.status : "disabled"}
            </span>
            <span className={`chip chip-${srv.callLevel}`}>{srv.callLevel}</span>
          </div>
          <p className="connection-scopes">
            {srv.transport === "stdio"
              ? `Executable: ${srv.command ?? ""} ${srv.args.join(" ")} ${srv.cwd ? `· cwd: ${srv.cwd}` : ""}${srv.envNames.length ? ` · env: ${srv.envNames.join(", ")}` : ""}`
              : `URL: ${srv.url ?? ""}`}
          </p>
          <div className="connection-actions">
            <button className="btn small" disabled={busy !== null} onClick={() => void act(`mcp_enable_${srv.id}`, () => mcpSetEnabled(srv.id, !srv.enabled), srv.enabled ? "Disabled." : "Enabled — validated on demand.")}>
              {srv.enabled ? "Disable" : "Enable"}
            </button>
            <button className="btn small ghost" disabled={busy !== null} onClick={() => void validateMcp(srv.id)}>
              Validate
            </button>
            <button className="btn small ghost" disabled={busy !== null} onClick={() => void inspectMcp(srv.id)}>
              Inspect tools
            </button>
            <button
              className="btn small ghost"
              disabled={busy !== null}
              onClick={() => void act(`mcp_level_${srv.id}`, () => mcpUpdate({ id: srv.id, callLevel: srv.callLevel === "read" ? "write" : "read" }), `Call level now ${srv.callLevel === "read" ? "write" : "read"}.`)}
            >
              {srv.callLevel === "read" ? "Allow writes" : "Read-only"}
            </button>
            <button className="btn small ghost" disabled={busy !== null} onClick={() => void act(`mcp_remove_${srv.id}`, () => mcpRemove(srv.id), "Removed.")}>
              Remove
            </button>
          </div>
          {srv.lastError && <p className="connections-error">{srv.lastError}</p>}
          {inspect[srv.id] && <p className="connections-msg">{inspect[srv.id]}</p>}
          {messages[`mcp_validate_${srv.id}`] && <p className="connections-error">{messages[`mcp_validate_${srv.id}`]}</p>}
        </div>
      ))}

      <div className="card connection-card">
        <div className="connection-head"><strong>Add MCP server</strong></div>
        <div className="connections-form">
          <select
            aria-label="Transport"
            value={draft.transport}
            onChange={(e) => setDraft({ ...draft, transport: e.target.value as "stdio" | "http" })}
          >
            <option value="stdio">Local stdio executable</option>
            <option value="http">Remote HTTPS (streamable HTTP)</option>
          </select>
          <input placeholder="Unique name" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
          {draft.transport === "stdio" ? (
            <>
              <input placeholder="Command (executable path)" value={draft.command} onChange={(e) => setDraft({ ...draft, command: e.target.value })} />
              <input placeholder="Arguments (space-separated)" value={draft.argsText} onChange={(e) => setDraft({ ...draft, argsText: e.target.value })} />
              <input placeholder="Working directory (optional)" value={draft.cwd} onChange={(e) => setDraft({ ...draft, cwd: e.target.value })} />
              <input placeholder="Env var names, comma-separated (values stored securely later)" value={draft.envText} onChange={(e) => setDraft({ ...draft, envText: e.target.value })} />
            </>
          ) : (
            <input placeholder="https://… HTTPS base URL" value={draft.url} onChange={(e) => setDraft({ ...draft, url: e.target.value })} />
          )}
          <select aria-label="Call permission level" value={draft.callLevel} onChange={(e) => setDraft({ ...draft, callLevel: e.target.value })}>
            {LEVELS.map((lv) => (
              <option key={lv.key} value={lv.key}>{lv.label}</option>
            ))}
          </select>
          <button className="btn small" disabled={busy !== null || !draft.name.trim()} onClick={() => void addMcp()}>
            Add server
          </button>
        </div>
        {messages.mcp_add && <p className="connections-error">{messages.mcp_add}</p>}
      </div>

      <h3 className="section-title">Audit log (latest 12)</h3>
      <div className="card connection-card">
        {audit.length === 0 ? (
          <p className="connections-msg">No audit events yet.</p>
        ) : (
          <ul className="connections-audit">
            {audit.map((ev) => (
              <li key={ev.id}>
                <code>{new Date(ev.ts).toLocaleString()}</code> <strong>{ev.provider ?? ev.category}</strong> · {ev.action} → {ev.outcome}
              </li>
            ))}
          </ul>
        )}
      </div>
      <p className="connections-note">
        Secrets never reach this page or the model context — they are stored in the OS credential store and used only by backend connector code.
      </p>
    </div>
  );
}
