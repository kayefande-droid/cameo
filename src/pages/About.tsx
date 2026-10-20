import React from "react";
import { getVersion } from "@tauri-apps/api/app";

export default function About() {
  const [version, setVersion] = React.useState<string>("(runtime unknown)");
  React.useEffect(() => {
    void getVersion().then(setVersion).catch(() => setVersion("(unavailable in dev)"));
  }, []);
  return (
    <div className="card" style={{ display: "flex", gap: 16, alignItems: "center", maxWidth: 620 }}>
      <img src="/logo.svg" alt="CAMEO emblem" width={64} height={64} />
      <div>
        <h2>CAMEO AI</h2>
        <p className="note" style={{ marginTop: 4 }}>
          Cognitive Autonomous Machine Executive Operator
        </p>
        <p className="note" style={{ marginTop: 4 }}>Version {version}</p>
        <p className="note" style={{ marginTop: 10, fontStyle: "italic" }}>
          “One intelligence. Multiple agents. Complete control.”
        </p>
        <p className="note" style={{ marginTop: 10 }}>
          Native desktop build: Tauri 2 + React + TypeScript + Rust. Provider traffic is made from
          Rust with keys held in Windows Credential Manager. See each feature page for its honest
          implemented/disabled status.
        </p>
      </div>
    </div>
  );
}
