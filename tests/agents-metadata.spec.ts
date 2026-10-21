import { describe, expect, it } from "vitest";
import { AGENTS, agentById } from "../src/types/tasks";
import { PROVIDERS } from "../src/types/ai";

describe("agent roster", () => {
  it("declares the operations agents then the advisory specialists", () => {
    expect(AGENTS.map((a) => a.codename)).toEqual([
      "CAMEO",
      "ATLAS",
      "FORGE",
      "PIXEL",
      "SENTINEL",
      "COUNSEL",
      "MEDIC",
      "LEDGER",
    ]);
  });
  it("every agent has a role, non-empty description and a system prompt", () => {
    for (const a of AGENTS) {
      expect(a.role.length).toBeGreaterThan(3);
      expect(a.description.length).toBeGreaterThan(10);
      expect(a.prompt.length).toBeGreaterThan(40);
    }
  });
  it("advisory agents carry the not-professional-advice boundary", () => {
    const advisory = AGENTS.filter((a) => a.advisory);
    expect(advisory.map((a) => a.id)).toEqual(["counsel", "medic", "ledger"]);
    for (const a of advisory) {
      expect(a.prompt).toContain("not a substitute for a licensed professional");
      expect(a.prompt).not.toMatch(/you are a (lawyer|physician|financial adviser)/i);
    }
  });
  it("resolves a known id and falls back to CAMEO for anything else", () => {
    expect(agentById("forge").codename).toBe("FORGE");
    expect(agentById("missing" as never).codename).toBe("CAMEO");
  });
});

describe("provider metadata", () => {
  it("covers OpenRouter, NVIDIA NIM, Ollama and Compatible", () => {
    expect(PROVIDERS.map((p) => p.id)).toEqual(["openrouter", "nvidia", "ollama", "compatible"]);
  });
  it("local Ollama does not require a key", () => {
    expect(PROVIDERS.find((p) => p.id === "ollama")!.requiresKey).toBe(false);
    expect(PROVIDERS.filter((p) => p.requiresKey).map((p) => p.id)).toEqual([
      "openrouter",
      "nvidia",
      "compatible",
    ]);
  });
});
