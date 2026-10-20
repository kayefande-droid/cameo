import AgentAvatar from "../components/AgentAvatar";
import { AGENTS } from "../types/tasks";

export default function Agents() {
  return (
    <>
      <div className="card agent-roster-intro">
        <AgentAvatar agent="cameo" size={72} />
        <div>
          <h3>CAMEO — Chief of Staff</h3>
          <p className="note" style={{ marginTop: 5 }}>
            Coordinates task plans, specialists, permissions, and verified outcomes. A model response
            never grants permission to perform a system action.
          </p>
        </div>
      </div>
      <div className="grid-2 agent-roster">
        {AGENTS.filter((a) => a.id !== "cameo").map((agent) => (
          <article key={agent.id} className={`card agent-card agent-card-${agent.id}`}>
            <AgentAvatar agent={agent.id} size={72} />
            <div className="agent-card-copy">
              <h3>{agent.codename}</h3>
              <span className="pill accent">{agent.role}</span>
              <p className="note" style={{ marginTop: 8 }}>{agent.description}</p>
              <span className="agent-ready"><span className="dot" /> Role configured · no autonomous process running</span>
            </div>
          </article>
        ))}
      </div>
      <div className="card">
        <h3>Safety model</h3>
        <ul className="note" style={{ paddingLeft: 18, marginTop: 6, lineHeight: 1.7 }}>
          <li>Web content, files and tool output are treated as untrusted data.</li>
          <li>Destructive or irreversible actions require your explicit approval.</li>
          <li>Avatars are visual identities only; they do not claim a running agent or fabricated progress.</li>
        </ul>
      </div>
    </>
  );
}
