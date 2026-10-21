import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn(async (..._args: unknown[]) => ({}) as unknown);

vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

import { execRun, isNeedsConfirmation, netFetch, operatorSetRoot } from "../src/services/operator";

describe("operator service", () => {
  beforeEach(() => invoke.mockClear());

  it("sends the program, args and approval flag by camelCase contract", async () => {
    invoke.mockResolvedValueOnce({
      exitCode: 0,
      stdout: "ok",
      stderr: "",
      truncated: false,
      timedOut: false,
      durationMs: 12,
      cwd: "C:\\proj",
    });

    const result = await execRun({ command: "cargo", args: ["test"], cwd: "src", approved: true });

    expect(invoke).toHaveBeenCalledWith("exec_run", {
      command: "cargo",
      args: ["test"],
      cwd: "src",
      approved: true,
    });
    expect(result.exitCode).toBe(0);
  });

  it("defaults to unapproved with no args or cwd", async () => {
    invoke.mockResolvedValueOnce({});
    await execRun({ command: "git" });
    expect(invoke).toHaveBeenCalledWith("exec_run", {
      command: "git",
      args: [],
      cwd: null,
      approved: false,
    });
  });

  it("recognises the backend's needs-confirmation refusal", () => {
    expect(isNeedsConfirmation(new Error("NEEDS_CONFIRMATION: 'rm' looks like recursive/forced delete."))).toBe(true);
    expect(isNeedsConfirmation("path 'x' is outside the project root")).toBe(false);
  });

  it("pins the project root and fetches through the same boundary", async () => {
    invoke.mockResolvedValueOnce({ enabled: true, root: "C:\\proj", net: true, device: false });
    await operatorSetRoot("C:\\proj");
    expect(invoke).toHaveBeenCalledWith("operator_set_root", { path: "C:\\proj" });

    invoke.mockResolvedValueOnce({ status: 200, url: "https://example.com", contentType: "text/html", truncated: false, bytes: 5, body: "hello" });
    await netFetch("https://example.com");
    expect(invoke).toHaveBeenCalledWith("net_fetch", { url: "https://example.com" });
  });
});
