import { useEffect, useRef, useState } from "react";

export default function Voice() {
  // ---- speech synthesis settings ----
  const [voices, setVoices] = useState<SpeechSynthesisVoice[]>([]);
  const [voiceIdx, setVoiceIdx] = useState(0);
  const [rate, setRate] = useState(1);
  const [volume, setVolume] = useState(1);
  const [text, setText] = useState("CAMEO reporting status. All systems nominal.");
  const [speakError, setSpeakError] = useState<string | null>(null);

  // ---- push-to-talk recording ----
  const [recording, setRecording] = useState(false);
  const [recError, setRecError] = useState<string | null>(null);
  const [recBlobUrl, setRecBlobUrl] = useState<string | null>(null);
  const recorderRef = useRef<MediaRecorder | null>(null);
  const chunksRef = useRef<Blob[]>([]);

  useEffect(() => {
    const load = () => setVoices(window.speechSynthesis?.getVoices() ?? []);
    load();
    window.speechSynthesis?.addEventListener("voiceschanged", load);
    return () => window.speechSynthesis?.removeEventListener("voiceschanged", load);
  }, []);

  const speak = () => {
    setSpeakError(null);
    const synth = window.speechSynthesis;
    if (!synth) {
      setSpeakError("Speech synthesis is not available in this WebView.");
      return;
    }
    synth.cancel();
    const u = new SpeechSynthesisUtterance(text);
    if (voices[voiceIdx]) u.voice = voices[voiceIdx];
    u.rate = rate;
    u.volume = volume;
    u.onend = () => setSpeakError(null);
    u.onerror = (ev) => setSpeakError(`Speech error: ${ev.error}`);
    synth.speak(u);
  };

  const stop = () => {
    window.speechSynthesis?.cancel();
  };

  const startRec = async () => {
    setRecError(null);
    setRecBlobUrl(null);
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      const rec = new MediaRecorder(stream);
      chunksRef.current = [];
      rec.ondataavailable = (e) => {
        if (e.data.size > 0) chunksRef.current.push(e.data);
      };
      rec.onstop = () => {
        stream.getTracks().forEach((t) => t.stop());
        const blob = new Blob(chunksRef.current, { type: rec.mimeType || "audio/webm" });
        setRecBlobUrl(URL.createObjectURL(blob));
      };
      rec.start();
      recorderRef.current = rec;
      setRecording(true);
    } catch (e) {
      setRecError(`Microphone error: ${e instanceof DOMException ? e.name : String(e)}`);
    }
  };

  const stopRec = () => {
    recorderRef.current?.stop();
    recorderRef.current = null;
    setRecording(false);
  };

  const sttSupported =
    typeof window !== "undefined" &&
    !!((window as any).SpeechRecognition || (window as any).webkitSpeechRecognition);

  return (
    <>
      <div className="card">
        <h3>
          <span className="section-title">Speech output (text-to-speech)</span>
          <span className="pill accent">works offline</span>
        </h3>
        <label className="field" style={{ marginTop: 10 }}>
          Text
          <textarea rows={2} value={text} onChange={(e) => setText(e.target.value)} />
        </label>
        <div style={{ display: "flex", gap: 10, marginTop: 10, flexWrap: "wrap", alignItems: "flex-end" }}>
          <label className="field" style={{ width: 280 }}>
            Voice
            <select value={voiceIdx} onChange={(e) => setVoiceIdx(parseInt(e.target.value, 10))}>
              {voices.length === 0 && <option value={0}>No voices detected yet</option>}
              {voices.map((v, i) => (
                <option key={`${v.name}${i}`} value={i}>
                  {v.name} ({v.lang})
                </option>
              ))}
            </select>
          </label>
          <label className="field" style={{ width: 140 }}>
            Speed ({rate.toFixed(2)}×)
            <input type="range" min={0.5} max={2} step={0.05} value={rate} onChange={(e) => setRate(parseFloat(e.target.value))} />
          </label>
          <label className="field" style={{ width: 140 }}>
            Volume ({Math.round(volume * 100)}%)
            <input type="range" min={0} max={1} step={0.05} value={volume} onChange={(e) => setVolume(parseFloat(e.target.value))} />
          </label>
          <button className="btn primary" onClick={speak}>Speak</button>
          <button className="btn" onClick={stop}>Stop speaking</button>
        </div>
        {speakError && <div className="note danger" style={{ marginTop: 8 }}>{speakError}</div>}
      </div>

      <div className="card">
        <h3>
          <span className="section-title">Push-to-talk (microphone recording)</span>
          <span className={`pill ${recording ? "danger" : "muted"}`}>
            <span className="dot" />
            {recording ? "recording — mic is live" : "idle"}
          </span>
        </h3>
        <div style={{ display: "flex", gap: 8, marginTop: 10 }}>
          <button className="btn primary" onClick={startRec} disabled={recording}>
            Start recording (push-to-talk)
          </button>
          <button className="btn" onClick={stopRec} disabled={!recording}>
            Stop
          </button>
        </div>
        {recording && <div className="note" style={{ marginTop: 8, color: "var(--danger)" }}>● Recording… press Stop to end.</div>}
        {recBlobUrl && (
          <audio controls src={recBlobUrl} style={{ marginTop: 10, width: 420 }} />
        )}
        {recError && <div className="note danger" style={{ marginTop: 8 }}>{recError}</div>}
      </div>

      <div className="card">
        <h3>Honest status</h3>
        <ul className="note" style={{ paddingLeft: 18, marginTop: 6, lineHeight: 1.7 }}>
          <li><strong>Implemented:</strong> text-to-speech (voice/rate/volume/stop) and opt-in push-to-talk recording via the WebView Media APIs.</li>
          <li>
            <strong>Speech-to-text:</strong>{" "}
            {sttSupported
              ? "a SpeechRecognition implementation is present in this WebView build."
              : "this WebView build exposes no speech-to-text engine — CAMEO does not fake one."}
          </li>
          <li>Wake word recognition is disabled until a local engine is bundled; microphone never stays hot by default.</li>
        </ul>
      </div>
    </>
  );
}
