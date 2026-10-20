import { invoke } from "@tauri-apps/api/core";
import type {
  AuditEvent,
  ConnectionStatusRow,
  GatewayProviderId,
  McpServer,
  McpValidateResult,
  Permissions,
  ProviderRegistryEntry,
} from "../types/gateway";

// Note: Tauri v2 maps camelCase JS arg names to snake_case Rust parameters,
// so we spell those args in camelCase with confidence in backend matching.js

// ---------- gateway status & registry ----------

export function gwRegistry() {
  return invoke<ProviderRegistryEntry[]>("gw_registry");
}

export function gwStatus(provider: GatewayProviderId | string) {
  return invoke<ConnectionStatusRow>("gw_status", { provider });
}

export function gwListAll() {
  return invoke<Array<{ provider: string; status: string; permissions: Partial<Permissions>; lastCheckOk: number | null; lastError: string | null }>>("gw_list_all");
}

// ---------- permissions ----------

export function gwGetPermissions(provider: GatewayProviderId | string) {
  return invoke<Partial<Permissions>>("gw_get_permissions", { provider });
}

export function gwSetPermissions(provider: GatewayProviderId | string, permissions: Partial<Permissions>) {
  return invoke<unknown>("gw_set_permissions", { provider, permissions });
}

// ---------- credentials (stored in the OS credential store) ----------

export function gwSetSecret(provider: GatewayProviderId | string, slot: string, secret: string) {
  return invoke<boolean>("gw_set_secret", { provider, slot, secret });
}

export function gwHasSecret(provider: GatewayProviderId | string, slot: string) {
  return invoke<boolean>("gw_has_secret", { provider, slot });
}

export function gwClearSecrets(provider: GatewayProviderId | string, slots: string[]) {
  return invoke<number>("gw_clear_secrets", { provider, slots });
}

// ---------- connect / refresh / test / disconnect ----------

export function gwOauthConnect(provider: GatewayProviderId, features: string[]) {
  return invoke<{ started: boolean; port?: number; authorizeUrl?: string } | Record<string, unknown>>("gw_oauth_connect", {
    provider,
    features,
  });
}

export function gwOauthRefresh(provider: GatewayProviderId) {
  return invoke<Record<string, unknown>>("gw_oauth_refresh", { provider });
}

export function gwTestConnection(provider: GatewayProviderId | string) {
  return invoke<{ ok?: boolean; message?: string } & Record<string, unknown>>("gw_test_connection", { provider });
}

export function gwConnectApiKey(provider: GatewayProviderId, apiKey: string) {
  return invoke<{ stored: boolean; check?: unknown }>("gw_connect_api_key", { provider, apiKey });
}

export function gwConnectorCall(
  provider: GatewayProviderId,
  tool: string,
  args: Record<string, unknown>,
  approved?: boolean,
) {
  return invoke<Record<string, unknown>>("gw_connector_call", { provider, tool, args, approved: approved ?? null });
}

export function gwDisconnect(provider: GatewayProviderId | string) {
  return invoke<Record<string, unknown>>("gw_disconnect", { provider });
}

// ---------- global Stop control ----------

export function gwStopControl(engage: boolean) {
  return invoke<Record<string, unknown>>("gw_stop_control", { engage });
}

export function gwStopStatus() {
  return invoke<{ stop: boolean }>("gw_stop_status");
}

// ---------- audit ----------

export function gwAuditList(limit?: number) {
  return invoke<AuditEvent[]>("gw_audit_list", { limit: limit ?? null });
}

// ---------- MCP servers ----------

export function mcpAdd(input: {
  name: string;
  transport: "stdio" | "http";
  command?: string | null;
  args?: string[] | null;
  cwd?: string | null;
  url?: string | null;
  env?: string[] | null;
  callLevel?: string | null;
}) {
  return invoke<McpServer>("mcp_add", {
    name: input.name,
    transport: input.transport,
    command: input.command ?? null,
    args: input.args ?? null,
    cwd: input.cwd ?? null,
    url: input.url ?? null,
    env: input.env ?? null,
    callLevel: input.callLevel ?? null,
  });
}

export function mcpList() {
  return invoke<McpServer[]>("mcp_list");
}

export function mcpUpdate(input: {
  id: string;
  name?: string | null;
  command?: string | null;
  args?: string[] | null;
  cwd?: string | null;
  url?: string | null;
  env?: string[] | null;
  callLevel?: string | null;
}) {
  return invoke<McpServer>("mcp_update", {
    id: input.id,
    name: input.name ?? null,
    command: input.command ?? null,
    args: input.args ?? null,
    cwd: input.cwd ?? null,
    url: input.url ?? null,
    env: input.env ?? null,
    callLevel: input.callLevel ?? null,
  });
}

export function mcpRemove(id: string) {
  return invoke<boolean>("mcp_remove", { id });
}

export function mcpSetEnabled(id: string, enabled: boolean) {
  return invoke<McpServer>("mcp_set_enabled", { id, enabled });
}

export function mcpValidate(id: string) {
  return invoke<McpValidateResult>("mcp_validate", { id });
}

export function mcpInspectTools(id: string) {
  return invoke<{ tools: McpValidateResult["tools"]; lastCheckedAt: number | null }>("mcp_inspect_tools", { id });
}

export function mcpCallTool(id: string, tool: string, args: Record<string, unknown>, approved?: boolean) {
  return invoke<Record<string, unknown>>("mcp_call_tool", { id, tool, args, approved: approved ?? null });
}
