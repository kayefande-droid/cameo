import { beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import Power from "../src/pages/Power";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

import { invoke } from "@tauri-apps/api/core";
const mockInvoke = invoke as unknown as Mock;

beforeEach(() => {
  mockInvoke.mockReset();
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === "power_status") {
      return Promise.resolve({ machine: "TEST-PC", os_version: "[Version 10.0.22621.6060]" });
    }
    return Promise.resolve({ ok: true, message: "ok", mechanism: "test", cancellable: false });
  });
  void cleanup();
});

describe("Power & Device Center", () => {
  it("never calls power_action without the user confirming", async () => {
    render(<Power />);
    await waitFor(() => expect(screen.getByText(/TEST-PC/)).toBeTruthy());
    const calls = mockInvoke.mock.calls.filter(([c]) => c === "power_action");
    expect(calls.length).toBe(0);
    cleanup();
  });

  it("shows a confirmation dialog before lock and passes confirmed=true", async () => {
    render(<Power />);
    await waitFor(() => expect(screen.getByText(/TEST-PC/)).toBeTruthy());

    fireEvent.click(screen.getByRole("button", { name: "Lock Windows" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toBeTruthy();
    // Still nothing executed — dialog is up.
    expect(
      mockInvoke.mock.calls.filter(([c]) => c === "power_action").length
    ).toBe(0);

    fireEvent.click(screen.getByRole("button", { name: "Lock" }));
    await waitFor(() => {
      const calls = mockInvoke.mock.calls.filter(([c]) => c === "power_action");
      expect(calls.length).toBe(1);
    });
    const params = (mockInvoke.mock.calls.find(([c]) => c === "power_action") as unknown[])[1] as { params: { action: string; confirmed: boolean } };
    expect(params.params.action).toBe("lock");
    expect(params.params.confirmed).toBe(true);
    cleanup();
  });

  it("cancel dialog aborts without invoking power_action", async () => {
    render(<Power />);
    await waitFor(() => expect(screen.getByText(/TEST-PC/)).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "Shut down" }));
    await screen.findByRole("dialog");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(
      mockInvoke.mock.calls.filter(([c]) => c === "power_action").length
    ).toBe(0);
    cleanup();
  });
});
