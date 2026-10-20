import { describe, expect, it, vi } from "vitest";

describe("CAMEO capability registry (adapted from ECC)", () => {
  it("validates and exposes every bundled capability", async () => {
    const { getCapabilityRegistry, listCapabilities } = await import("../src/services/capabilities");
    const registry = getCapabilityRegistry();
    expect(registry.schema).toBe("cameo.capability-registry.v1");
    expect(registry.source.upstreamLicense).toContain("MIT");
    expect(listCapabilities().length).toBeGreaterThan(5);
    expect(listCapabilities().every((entry) => entry.tools.every((t) =>
      [
        "provider_chat",
        "bounded_project_snapshot",
        "review_workers",
        "allowlisted_checks",
        "file_proposals",
        "developer_probes",
        "memory_store",
      ].includes(t),
    ))).toBe(true);
  });

  it("selects only active capabilities for the requesting agent and phase", async () => {
    const { selectCapabilitiesFor } = await import("../src/services/capabilities");
    expect(selectCapabilitiesFor("forge", "implement").map((c) => c.id)).toEqual(["tdd-workflow"]);
    expect(selectCapabilitiesFor("sentinel", "review").map((c) => c.id)).toEqual(["security-review"]);
    expect(selectCapabilitiesFor("forge", "remember")).toEqual([]);
  });

  it("pulls in declared dependencies without duplicates", async () => {
    const { capabilityPhasesForStatus } = await import("../src/services/capabilities");
    const entries = capabilityPhasesForStatus("forge", "partially_completed");
    expect(entries.map((c) => c.id)).toEqual(["build-error-repair", "verification-loop"]);
  });

  it("maps task statuses to phases and returns nothing for terminal states", async () => {
    const { capabilityPhasesForStatus } = await import("../src/services/capabilities");
    expect(capabilityPhasesForStatus("atlas", "queued").map((c) => c.id)).toEqual(["plan-acceptance-criteria"]);
    expect(capabilityPhasesForStatus("atlas", "completed")).toEqual([]);
  });

  it("builds a single prompt-gate text per specialist", async () => {
    const { capabilityGuidanceText } = await import("../src/services/capabilities");
    const text = capabilityGuidanceText("sentinel");
    expect(text).toContain("security-review: ");
    expect(text).toContain("read-only");
  });

  it("refuses to serve guidance when the bundled registry becomes corrupt", async () => {
    vi.doMock("../src/data/capabilities.json", () => ({
      default: {
        schema: "wrong-schema",
        source: {},
        selection: {},
        capabilities: [{ id: "bad", agent: "unknown-agent", phase: "plan", description: "", tools: [], permissions: [], dependencies: [], availability: "active" }],
      },
    }));
    vi.resetModules();
    const { getCapabilityRegistry, listCapabilities } = await import("../src/services/capabilities");
    expect(() => getCapabilityRegistry()).toThrow(/failed validation/);
    expect(() => listCapabilities()).toThrow(/failed validation/);
    vi.doUnmock("../src/data/capabilities.json");
    vi.resetModules();
  });
});
