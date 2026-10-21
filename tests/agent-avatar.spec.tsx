import { describe, expect, it } from "vitest";
import { render, screen, within } from "@testing-library/react";
import AgentAvatar from "../src/components/AgentAvatar";
import { AGENTS } from "../src/types/tasks";

describe("agent avatars", () => {
  it("renders an accessible animated avatar for every configured agent", () => {
    const { container } = render(
      <div>
        {AGENTS.map((agent) => (
          <AgentAvatar key={agent.id} agent={agent.id} />
        ))}
      </div>
    );

    for (const agent of AGENTS) {
      expect(screen.getByRole("img", { name: `${agent.codename} animated AI avatar` })).toBeTruthy();
      expect(container.querySelector(`.agent-avatar-${agent.id}.is-animated`)).toBeTruthy();
    }
    expect(container.querySelectorAll("svg.agent-avatar")).toHaveLength(AGENTS.length);
  });

  it("supports a static avatar when animation is explicitly disabled", () => {
    const { container } = render(<AgentAvatar agent="sentinel" animate={false} />);
    expect(within(container).getByRole("img", { name: "SENTINEL animated AI avatar" })).toBeTruthy();
    expect(container.querySelector(".agent-avatar-sentinel.is-animated")).toBeNull();
  });
});
