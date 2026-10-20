import type { CSSProperties } from "react";
import type { AgentId } from "../types/tasks";

export type AgentActivity = "idle" | "listening" | "thinking" | "working" | "approval" | "error" | "completed";

const AVATAR: Record<AgentId, { initials: string; name: string; color: string; accent: string; symbol: string }> = {
  cameo: { initials: "C", name: "CAMEO", color: "#22D3EE", accent: "#8B5CF6", symbol: "✦" },
  atlas: { initials: "A", name: "ATLAS", color: "#60A5FA", accent: "#22D3EE", symbol: "◇" },
  forge: { initials: "F", name: "FORGE", color: "#FB923C", accent: "#FBBF24", symbol: "⌘" },
  pixel: { initials: "P", name: "PIXEL", color: "#F472B6", accent: "#8B5CF6", symbol: "▦" },
  sentinel: { initials: "S", name: "SENTINEL", color: "#34D399", accent: "#22D3EE", symbol: "⬡" },
};

export default function AgentAvatar({
  agent,
  size = 64,
  animate = true,
  label,
  activity = "idle",
  audioLevel = 0,
}: {
  agent: AgentId;
  size?: number;
  animate?: boolean;
  label?: string;
  activity?: AgentActivity;
  audioLevel?: number;
}) {
  const meta = AVATAR[agent];
  return (
    <svg
      className={`agent-avatar agent-avatar-${agent}${animate ? " is-animated" : ""} agent-activity-${activity}`}
      width={size}
      height={size}
      viewBox="0 0 80 80"
      role="img"
      aria-label={label ?? `${meta.name} animated AI avatar`}
      style={{ "--avatar-color": meta.color, "--avatar-accent": meta.accent, "--audio-level": Math.max(0, Math.min(1, audioLevel)) } as CSSProperties}
    >
      <defs>
        <radialGradient id={`avatar-gradient-${agent}`} cx="35%" cy="28%" r="72%">
          <stop offset="0" stopColor={meta.color} stopOpacity=".34" />
          <stop offset="1" stopColor={meta.accent} stopOpacity=".06" />
        </radialGradient>
        <filter id={`avatar-glow-${agent}`} x="-60%" y="-60%" width="220%" height="220%">
          <feGaussianBlur stdDeviation="2.2" result="blur" />
          <feMerge><feMergeNode in="blur" /><feMergeNode in="SourceGraphic" /></feMerge>
        </filter>
      </defs>
      <circle className="avatar-halo" cx="40" cy="40" r="35" fill={`url(#avatar-gradient-${agent})`} />
      <circle className="avatar-orbit avatar-orbit-outer" cx="40" cy="40" r="34" fill="none" stroke={meta.color} strokeOpacity=".48" strokeWidth="1" strokeDasharray="2 5" />
      <circle className="avatar-orbit avatar-orbit-inner" cx="40" cy="40" r="27" fill="none" stroke={meta.accent} strokeOpacity=".58" strokeWidth="1" strokeDasharray="11 7 2 7" />
      <path className="avatar-shell" d="M40 17 58 27.5v21L40 59 22 48.5v-21z" fill="#0B1020" fillOpacity=".78" stroke={meta.color} strokeWidth="1.5" />
      <path d="M40 23 53 30.5v15L40 53l-13-7.5v-15z" fill={`url(#avatar-gradient-${agent})`} stroke={meta.accent} strokeOpacity=".7" />
      <circle className="avatar-core" cx="40" cy="39" r="11" fill={meta.color} fillOpacity=".15" stroke={meta.color} strokeWidth="1.2" filter={`url(#avatar-glow-${agent})`} />
      <text x="40" y="44" textAnchor="middle" fill="white" fontSize="13" fontWeight="700" fontFamily="Segoe UI, sans-serif">{meta.initials}</text>
      <text className="avatar-symbol" x="40" y="11" textAnchor="middle" fill={meta.accent} fontSize="8" fontFamily="Segoe UI Symbol, sans-serif">{meta.symbol}</text>
      <circle className="avatar-node" cx="14" cy="40" r="2" fill={meta.color} />
      <circle className="avatar-node avatar-node-delay" cx="66" cy="40" r="2" fill={meta.accent} />
      <circle className="avatar-attention" cx="65" cy="15" r="5" />
      <path className="avatar-error-mark" d="M65 12v4m0 2v.5" />
      <path className="avatar-complete-mark" d="m60 15 3 3 7-7" />
      <g className="avatar-audio-meter" aria-hidden="true">
        <rect x="35" y="63" width="2.5" height="5" rx="1.25" />
        <rect x="39" y="61" width="2.5" height="8" rx="1.25" />
        <rect x="43" y="63" width="2.5" height="5" rx="1.25" />
      </g>
    </svg>
  );
}
