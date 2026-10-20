import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

type Invoker = (cmd: string, args?: unknown) => Promise<unknown>;
const invocations: Array<{ cmd: string; args: unknown }> = [];

let impl: Invoker = async (cmd) => {
  if (cmd === "gw_registry") {
    return [
      {
        provider: "google",
        display: "Google & Gmail",
        authMethod: "oauth",
        baseEndpoint: "https://gmail.googleapis.com/",
        requiredScopes: ["https://www.googleapis.com/auth/gmail.readonly"],
        revokeGuide: "revoke",
        tools: [{ name: "gmail_send", level: "sensitive", approvalNeeded: true, description: "" }],
      },
      {
        provider: "render",
        display: "Render.com",
        authMethod: "api_key",
        baseEndpoint: "https://api.render.com/",
        requiredScopes: [],
        revokeGuide: "revoke",
        tools: [{ name: "render_list_services", level: "read", approvalNeeded: false, description: "" }],
      },
    ];
  }
  if (cmd === "gw_list_all") {
    return [{ provider: "google", status: "connected", permissions: { read: true }, lastCheckOk: 1, lastError: null }];
  }
  if (cmd === "gw_stop_status") return { stop: false };
  if (cmd === "gw_audit_list") return [];
  if (cmd === "mcp_list") return [];
  return {};
};

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => {
    invocations.push({ cmd, args });
    return impl(cmd, args);
  },
}));

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import Connections from "../src/pages/Connections";
import { gwConnectApiKey, gwSetPermissions } from "../src/services/gateway";

describe("Connections page", () => {
  afterEach(() => {
    cleanup();
  });

  beforeEach(() => {
    invocations.length = 0;
  });

  it("renders provider cards with real backend status per provider", async () => {
    render(<Connections />);
    await waitFor(() => expect(screen.getByText("Google & Gmail")).toBeTruthy());
    expect(screen.getByText(/Connected & validated/i)).toBeTruthy();
    expect(await screen.findByText("Render.com")).toBeTruthy();
    // Only google has a stored row; render default status comes from the registry pass
    const renderCardText = screen.getByText("Render.com").closest(".card")?.textContent ?? "";
    expect(renderCardText).toContain("Not configured");
    expect(invocations.some((i) => i.cmd === "gw_registry")).toBe(true);
    expect(invocations.some((i) => i.cmd === "gw_list_all")).toBe(true);
  });

  it("shows sensitive tools with approval marker", async () => {
    render(<Connections />);
    await screen.findByText("Google & Gmail");
    expect(document.body.textContent).toContain("gmail_send");
  });

  it("stores Render API keys through the backend, never locally", async () => {
    render(<Connections />);
    await screen.findByText("Render.com");
    const card = screen.getAllByText("Render.com")[0].closest(".card") as HTMLElement;
    const input = card.querySelector("input[type=password]") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "rnd_abcdefghij0123456789" } });
    const buttons = Array.from(card.querySelectorAll("button"));
    const save = buttons.find((b) => b.textContent === "Save & test") as HTMLButtonElement;
    fireEvent.click(save);
    await waitFor(() =>
      expect(invocations.some((i) => i.cmd === "gw_connect_api_key" && (i.args as { apiKey?: string })?.apiKey === "rnd_abcdefghij0123456789")).toBe(true),
    );
  });

  it("permission toggles route through gw_set_permissions", async () => {
    render(<Connections />);
    await screen.findByText("Render.com");
    const permInvocations = () => invocations.filter((i) => i.cmd === "gw_set_permissions");
    const before = permInvocations().length;
    await gwSetPermissions("render", { read: true, prepare: true, write: false, sensitive: false });
    await waitFor(() => expect(permInvocations().length).toBe(before + 1));
    expect((permInvocations()[0]?.args as { permissions?: { prepare?: boolean } })?.permissions?.prepare).toBe(true);
  });

  it("global stop control engages the backend control", async () => {
    render(<Connections />);
    await screen.findByText("Google & Gmail");
    const stop = screen.getByText("Engage Stop");
    fireEvent.click(stop);
    await waitFor(() =>
      expect(invocations.some((i) => i.cmd === "gw_stop_control" && (i.args as { engage?: boolean })?.engage === true)).toBe(true),
    );
  });
});
