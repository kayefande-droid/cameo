import { invoke } from "@tauri-apps/api/core";

export interface OperatorStatus {
  enabled: boolean;
  root: string | null;
  net: boolean;
  device: boolean;
  fullAccess?: boolean;
}

export interface ExecResult {
  exitCode: number;
  stdout: string;
  stderr: string;
  truncated: boolean;
  timedOut: boolean;
  durationMs: number;
  cwd: string;
}

export interface FetchResult {
  status: number;
  url: string;
  contentType: string;
  truncated: boolean;
  bytes: number;
  body: string;
}

export function operatorStatus() {
  return invoke<OperatorStatus>("operator_status");
}

export function operatorConfigure(input: { enabled: boolean; net: boolean; device: boolean }) {
  return invoke<OperatorStatus>("operator_configure", {
    enabled: input.enabled,
    net: input.net,
    device: input.device,
  });
}

/** Pin the single folder command execution is confined to. */
export function operatorSetRoot(path: string) {
  return invoke<OperatorStatus>("operator_set_root", { path });
}

/**
 * Explicit owner opt-in to full system scope (any folder, incl. drive
 * roots). Destructive-command confirmation and audit logging stay on.
 */
export function operatorSetFullAccess(full: boolean) {
  return invoke<OperatorStatus>("operator_set_full_access", { full });
}

/**
 * Run a program inside the selected project root. Destructive commands are
 * refused unless `approved` is set; the backend re-checks the pattern itself.
 */
export function execRun(input: {
  command: string;
  args?: string[];
  cwd?: string | null;
  approved?: boolean;
}) {
  return invoke<ExecResult>("exec_run", {
    command: input.command,
    args: input.args ?? [],
    cwd: input.cwd ?? null,
    approved: input.approved ?? false,
  });
}

/** GET a public https URL. Private/loopback addresses and non-https schemes are rejected. */
export function netFetch(url: string) {
  return invoke<FetchResult>("net_fetch", { url });
}

/** Hand a path or URL to the OS default handler. */
export function deviceLaunch(target: string) {
  return invoke<{ launched: boolean; target: string }>("device_launch", { target });
}

export function isNeedsConfirmation(error: unknown): boolean {
  return String(error).includes("NEEDS_CONFIRMATION");
}
