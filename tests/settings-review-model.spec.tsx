import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

const mocks = vi.hoisted(() => ({
  getPref: vi.fn(),
  setPref: vi.fn(),
  hasKey: vi.fn(),
  listModels: vi.fn(),
  invoke: vi.fn(),
}));

vi.mock("../src/services/store", () => ({ getPref: mocks.getPref, setPref: mocks.setPref }));
vi.mock("../src/services/ai", () => ({
  deleteKey: vi.fn(),
  hasKey: mocks.hasKey,
  listModels: mocks.listModels,
  saveKey: vi.fn(),
  testConnection: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

import Settings from "../src/pages/Settings";

afterEach(() => cleanup());
beforeEach(() => {
  vi.clearAllMocks();
  mocks.getPref.mockResolvedValue(null);
  mocks.setPref.mockResolvedValue(undefined);
  mocks.hasKey.mockResolvedValue(false);
  mocks.invoke.mockResolvedValue(undefined);
  mocks.listModels.mockResolvedValue([
    { id: "reviewer/model", name: "Reviewer", provider: "nvidia", capabilities: ["text", "streaming"] },
  ]);
});

describe("specialist review model settings", () => {
  it("selects a separate review model and can restore primary-model fallback", async () => {
    render(<Settings />);
    fireEvent.click(await screen.findByText("NVIDIA NIM"));
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    fireEvent.click(await screen.findByRole("button", { name: "Use reviewer/model for specialist reviews" }));

    await waitFor(() => expect(mocks.setPref).toHaveBeenCalledWith(
      "review_model_selection",
      JSON.stringify({ provider: "nvidia", model: "reviewer/model" }),
    ));
    expect(screen.getByText("Specialist review model: nvidia / reviewer/model")).toBeTruthy();
    expect(screen.getByText(/hosted model sends data to that provider/)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Use primary for reviews" }));
    await waitFor(() => expect(mocks.setPref).toHaveBeenLastCalledWith("review_model_selection", "primary"));
    expect(screen.getByText(/Specialist review model: primary/)).toBeTruthy();
  });
});
