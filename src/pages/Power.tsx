import { useEffect, useState } from "react";
import ConfirmDialog from "../components/Dialogs";
import { invoke } from "@tauri-apps/api/core";

interface ShutdownStatus {
  machine: string;
  os_version: string;
}

type Spec = {
  title: string;
  body: React.ReactNode;
  confirmLabel: string;
  danger?: boolean;
  onConfirm: () => void;
  onCancel?: () => void;
} | null;

interface PowerParams {
  action: string;
  delay_seconds: number;
  confirmed: boolean;
}

export default function Power() {
  const [spec, setSpec] = useState<Spec>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [sysInfo, setSysInfo] = useState<ShutdownStatus | null>(null);
  const [delay, setDelay] = useState(60);
  const [pendingAction, setPendingAction] = useState<string | null>(null);
  const [countdown, setCountdown] = useState<number | null>(null);

  useEffect(() => {
    void invoke<ShutdownStatus>("power_status")
      .then((r) => setSysInfo(r))
      .catch(() => setSysInfo(null));
  }, []);

  const run = async (params: PowerParams) => {
    setStatus("Executing…");
    try {
      const res = await invoke<{ ok: boolean; message: string; mechanism: string; cancellable: boolean }>(
        "power_action",
        { params }
      );
      setStatus(`${res.message}\nmechanism: ${res.mechanism}`);
      if (res.cancellable && (params.action === "shutdown" || params.action === "restart")) {
        startCountdown(params.delay_seconds);
      }
    } catch (e) {
      setStatus(`✖ ${String(e)}`);
    }
  };

  const startCountdown = (secs: number) => {
    let remaining = secs;
    setCountdown(remaining);
    const t = setInterval(() => {
      remaining -= 1;
      setCountdown(remaining);
      if (remaining <= 0) {
        clearInterval(t);
        setCountdown(null);
      }
    }, 1000);
  };

  const cancelPending = async () => {
    setStatus("Cancelling scheduled shutdown…");
    try {
      const res = await invoke<{ message: string }>("power_action", {
        params: { action: "cancel", delay_seconds: 0, confirmed: false },
      });
      setStatus(res.message);
    } catch (e) {
      setStatus(`✖ ${String(e)}`);
    }
    setCountdown(null);
  };

  const request = (action: "shutdown" | "restart" | "sleep" | "lock") => {
    setPendingAction(action);
    setSpec({
      title: action === "sleep" ? "Put this PC to sleep?" : action === "lock" ? "Lock Windows now?" : action === "shutdown" ? "Shut down this PC?" : "Restart this PC?",
      body:
        (action === "sleep" || action === "lock")
          ? "This uses a documented Windows mechanism (rundll32). Lock keeps all your apps open."
          : "Unsaved work in other apps may be lost. A countdown lets you cancel the action before it happens.",
      confirmLabel: action === "shutdown" ? "Shut down" : action === "restart" ? "Restart" : action === "sleep" ? "Sleep" : "Lock",
      danger: action === "shutdown" || action === "restart",
      onConfirm: () => {
        setSpec(null);
        void run({
          action,
          delay_seconds: delay,
          confirmed: true,
        });
      },
      onCancel: () => {
        setSpec(null);
        setPendingAction(null);
      },
    });
  };

  return (
    <>
      <div className="card">
        <h3>System</h3>
        <p className="note mono" style={{ marginTop: 6 }}>
          Host: {sysInfo?.machine ?? "unknown"} · OS:{" "}
          {(sysInfo?.os_version ?? "unknown").replace(/\[|Version/g, "").trim() || "unknown"}
        </p>
        <div style={{ display: "flex", gap: 8, marginTop: 10 }}>
          <label className="field" style={{ width: 180 }}>
            Countdown before shutdown (s)
            <input
              type="number"
              min={5}
              max={3600}
              value={delay}
              onChange={(e) => setDelay(Math.max(5, parseInt(e.target.value || "60", 10)))}
            />
          </label>
        </div>
        <div style={{ display: "flex", gap: 8, marginTop: 12, flexWrap: "wrap" }}>
          <button className="btn danger" onClick={() => request("shutdown")}>Shut down</button>
          <button className="btn danger" onClick={() => request("restart")}>Restart</button>
          <button className="btn" onClick={() => request("sleep")}>Sleep</button>
          <button className="btn" onClick={() => request("lock")}>Lock Windows</button>
          <button className="btn ghost" onClick={cancelPending}>Cancel pending shutdown</button>
        </div>
        {countdown !== null && (
          <div className="pill warning" style={{ marginTop: 12 }}>
            <span className="dot" />
            {pendingAction} in {countdown}s — press “Cancel pending shutdown” to abort.
          </div>
        )}
        {status && <div className="note" style={{ whiteSpace: "pre-line", marginTop: 10 }}>{status}</div>}
        <p className="note" style={{ marginTop: 12 }}>
          Shutdown/restart use <span className="mono">shutdown.exe</span> with a countdown; sleep uses{" "}
          <span className="mono">SetSuspendState</span>; lock uses <span className="mono">LockWorkStation</span>.
          Every action requires your explicit confirmation — automated tests never trigger real power actions.
        </p>
      </div>
      {spec && <ConfirmDialog spec={spec} />}
    </>
  );
}
