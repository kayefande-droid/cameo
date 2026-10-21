import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import CameoDock from "../src/CameoDock";

afterEach(() => cleanup());

describe("CameoDock redesigned shell", () => {
  it("wires the composer to onSubmitPrompt and clears it", () => {
    const onSubmitPrompt = vi.fn();
    render(<CameoDock initialMode="workspace" onSubmitPrompt={onSubmitPrompt} />);
    const box = screen.getByLabelText("Message CAMEO");
    fireEvent.change(box, { target: { value: "  hello dock  " } });
    fireEvent.click(screen.getByRole("button", { name: "Submit message" }));
    expect(onSubmitPrompt).toHaveBeenCalledWith("hello dock");
    expect((box as HTMLTextAreaElement).value).toBe("");
  });

  it("stays a UI-only preview without a submit handler", () => {
    render(<CameoDock initialMode="workspace" />);
    expect(screen.getByText(/UI preview/)).toBeTruthy();
    const box = screen.getByLabelText("Message CAMEO");
    fireEvent.change(box, { target: { value: "typed" } });
    fireEvent.click(screen.getByRole("button", { name: "Submit message" }));
    expect((box as HTMLTextAreaElement).value).toBe("");
  });

  it("navigates through the grouped rail and reports the page", () => {
    const onNavigate = vi.fn();
    render(<CameoDock initialMode="workspace" onNavigate={onNavigate} />);
    expect(screen.getByText("Operate")).toBeTruthy();
    expect(screen.getByText("Build")).toBeTruthy();
    expect(screen.getByText("System")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Proxy" }));
    expect(onNavigate).toHaveBeenCalledWith("Proxy");
  });

  it("slots host content into the Assistant view", () => {
    render(
      <CameoDock initialMode="workspace">
        <div>real conversation history</div>
      </CameoDock>
    );
    expect(screen.getByText("real conversation history")).toBeTruthy();
    expect(screen.getByText("What can I help you build?")).toBeTruthy();
  });

  it("reports collapse from the workspace close control", () => {
    const onModeChange = vi.fn();
    render(<CameoDock initialMode="workspace" onModeChange={onModeChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Collapse dock" }));
    expect(onModeChange).toHaveBeenCalledWith("dock");
  });

  it("toggles pin state through the pill control", () => {
    const onPinToggle = vi.fn();
    render(<CameoDock initialMode="workspace" pinned={false} onPinToggle={onPinToggle} />);
    fireEvent.click(screen.getByRole("button", { name: "Snap to top center" }));
    fireEvent.click(screen.getByRole("button", { name: "Pin dock" }));
    expect(onPinToggle).toHaveBeenCalledWith(true);
  });
});
