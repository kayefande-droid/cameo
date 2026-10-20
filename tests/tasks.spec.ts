import { describe, expect, it } from "vitest";
import { canTransition, statusLabel, type TaskStatus } from "../src/types/tasks";

describe("task status transitions", () => {
  it("moves queued → planning", () => {
    expect(canTransition("queued", "planning")).toBe(true);
  });
  it("moves planned → awaiting approval → running → verifying → completed", () => {
    expect(canTransition("planning", "awaiting_approval")).toBe(true);
    expect(canTransition("awaiting_approval", "running")).toBe(true);
    expect(canTransition("running", "verifying")).toBe(true);
    expect(canTransition("verifying", "completed")).toBe(true);
  });
  it("allows cancel and fail from active phases", () => {
    expect(canTransition("running", "cancelled")).toBe(true);
    expect(canTransition("running", "failed")).toBe(true);
    expect(canTransition("awaiting_approval", "cancelled")).toBe(true);
  });
  it("rejects jumping straight to completed", () => {
    expect(canTransition("queued", "completed")).toBe(false);
    expect(canTransition("planning", "completed")).toBe(false);
  });
  it("terminal states accept nothing", () => {
    expect(canTransition("completed", "running")).toBe(false);
    expect(canTransition("cancelled", "completed")).toBe(false);
  });
  it("failed can be re-queued", () => {
    expect(canTransition("failed", "queued")).toBe(true);
  });
  it("labels use spaces", () => {
    expect(statusLabel("awaiting_approval" as TaskStatus)).toBe("awaiting approval");
  });
});
