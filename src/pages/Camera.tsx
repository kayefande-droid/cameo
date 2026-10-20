import { useCallback, useEffect, useRef, useState } from "react";

interface CameraDevice {
  deviceId: string;
  label: string;
}

export default function Camera() {
  const videoRef = useRef<HTMLVideoElement>(null);
  const [devices, setDevices] = useState<CameraDevice[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [permission, setPermission] = useState<"unknown" | "granted" | "denied">("unknown");
  const [error, setError] = useState<string | null>(null);

  const stopCamera = useCallback(() => {
    const v = videoRef.current;
    if (v?.srcObject instanceof MediaStream) {
      for (const track of v.srcObject.getTracks()) {
        track.stop();
      }
      v.srcObject = null;
    }
    setActive(null);
  }, []);

  useEffect(() => {
    void navigator.permissions
      ?.query({ name: "camera" as PermissionName })
      .then((res) => setPermission(res.state === "granted" ? "granted" : res.state === "denied" ? "denied" : "unknown"))
      .catch(() => setPermission("unknown"));
    void navigator.mediaDevices
      ?.enumerateDevices()
      .then((list) =>
        setDevices(
          list
            .filter((d) => d.kind === "videoinput")
            .map((d) => ({ deviceId: d.deviceId, label: d.label || "(unnamed camera)" }))
        )
      )
      .catch(() => setDevices([]));
    return () => stopCamera();
  }, [stopCamera]);

  const start = useCallback(
    async (deviceId?: string) => {
      setError(null);
      stopCamera();
      try {
        const stream = await navigator.mediaDevices.getUserMedia({
          video: deviceId ? { deviceId } : true,
          audio: false,
        });
        const v = videoRef.current;
        if (v) {
          v.srcObject = stream;
          setActive(deviceId ?? "default");
          setPermission("granted");
        }
      } catch (e) {
        const err = e as DOMException;
        setPermission(err.name === "NotAllowedError" || err.name === "PermissionDeniedError" ? "denied" : "unknown");
        setError(
          err.name === "NotAllowedError"
            ? "Camera access was denied in this WebView. Allow it in Windows Settings → Privacy → Camera, then try again."
            : `Camera error: ${err.name || String(e)}${err.message ? ` — ${err.message}` : ""}`
        );
      }
    },
    [stopCamera]
  );

  return (
    <>
      <div className="card">
        <h3>
          <span className="section-title">Opt-in camera preview</span>
          <span className={`pill ${active ? "success" : permission === "denied" ? "danger" : "muted"}`}>
            <span className="dot" />
            {active ? "live" : permission === "denied" ? "permission denied" : "off"}
          </span>
        </h3>
        <div className="camera-view" style={{ margin: "12px 0" }}>
          <video ref={videoRef} autoPlay playsInline muted />
        </div>
        <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
          <button
            className="btn primary"
            onClick={() => start(devices[0]?.deviceId)}
            disabled={devices.length === 0}
          >
            Start camera
          </button>
          <button className="btn" onClick={stopCamera} disabled={!active}>
            Stop
          </button>
          {devices.length > 1 && (
            <select
              style={{ width: 260 }}
              onChange={(e) => (e.target.value ? start(e.target.value) : undefined)}
              aria-label="Camera selection"
            >
              <option value="">Choose camera device…</option>
              {devices.map((d) => (
                <option key={d.deviceId} value={d.deviceId}>
                  {d.label}
                </option>
              ))}
            </select>
          )}
          <span className="note">
            {devices.length} camera device{devices.length === 1 ? "" : "s"} detected.
          </span>
        </div>
        {error && <div className="note danger" style={{ marginTop: 10 }}>{error}</div>}
      </div>
      <div className="card">
        <h3>Honest status</h3>
        <ul className="note" style={{ paddingLeft: 18, marginTop: 6, lineHeight: 1.7 }}>
          <li><strong>Implemented:</strong> device discovery, opt-in preview, start/stop and permission status.</li>
          <li><strong>Not claimed:</strong> face detection, face matching and Windows Hello integration are disabled in this build — WebView2 lacks a stable in-process face detection contract and Hello requires a Microsoft-signed app identity.</li>
          <li>Video frames never leave the window and are never sent to AI providers.</li>
        </ul>
      </div>
    </>
  );
}
