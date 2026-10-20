export type GatewayProviderId = "google" | "github" | "render";

export type PermissionLevel = "read" | "prepare" | "write" | "sensitive";

export interface Permissions {
  read: boolean;
  prepare: boolean;
  write: boolean;
  sensitive: boolean;
}

export type ConnectionStatus =
  | "not_configured"
  | "disconnected"
  | "connecting"
  | "awaiting_user_authorization"
  | "connected"
  | "connected_limited"
  | "expired"
  | "failed";

export interface GatewayTool {
  name: string;
  level: PermissionLevel;
  approvalNeeded: boolean;
  description: string;
}

export interface ProviderRegistryEntry {
  provider: GatewayProviderId;
  display: string;
  authMethod: "oauth" | "api_key";
  baseEndpoint: string;
  requiredScopes: string[];
  revokeGuide: string;
  tools: GatewayTool[];
}

export interface ConnectionStatusRow {
  authMethod: string;
  status: ConnectionStatus;
  accountLabel: string | null;
  scopes: string[];
  permissions: Partial<Permissions>;
  lastCheckAt: number | null;
  lastCheckOk: number | null;
  lastError: string | null;
  tokenExpiresAt: number | null;
  updatedAt?: number;
}

export interface AuditEvent {
  id: number;
  ts: number;
  category: string;
  provider: string | null;
  action: string;
  outcome: string;
  detail: Record<string, unknown>;
}

export type McpTransport = "stdio" | "http";

export interface McpServer {
  id: string;
  name: string;
  transport: McpTransport;
  command: string | null;
  args: string[];
  cwd: string | null;
  url: string | null;
  envNames: string[];
  enabled: boolean;
  callLevel: PermissionLevel;
  status: string;
  tools: unknown;
  lastCheckedAt: number | null;
  lastError: string | null;
  createdAt?: number;
  updatedAt?: number;
}

export interface McpValidateResult {
  validated: boolean;
  authRequirement?: { authRequired: boolean; resourceMetadataUrl?: string | null; status?: number; message: string };
  command?: string;
  args?: string[];
  cwd?: string;
  envNames?: string[];
  protocolVersion?: string;
  tools?: Array<{ name: string; schemaOk?: boolean; description?: string }>;
  error?: string;
}
