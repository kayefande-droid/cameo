import { describe, expect, it } from "vitest";
import { AGENTS } from "../src/types/tasks";
import { PROVIDERS } from "../src/types/ai";

describe("agent roster", () => {
  it("declares exactly five specialists including CAMEO", () => {
    expect(AGENTS.map((a) => a.codename)).toEqual([
      "CAMEO",
      "ATLAS",
      "FORGE",
      "PIXEL",
      "SENTINEL",
    ]);
  });
  it("every agent has a role and non-empty description", () => {
    for (const a of AGENTS) {
      expect(a.role.length).toBeGreaterThan(3);
      expect(a.description.length).toBeGreaterThan(10);
    }
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
