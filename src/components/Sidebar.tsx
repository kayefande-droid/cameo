
export type PageId =
  | "home"
  | "tasks"
  | "agents"
  | "connections"
  | "developer"
  | "proxy"
  | "browser"
  | "camera"
  | "voice"
  | "power"
  | "settings"
  | "about";

export const NAV_ITEMS: { id: PageId; label: string; icon: string }[] = [
  { id: "home", label: "Assistant", icon: "💬" },
  { id: "tasks", label: "Tasks", icon: "📋" },
  { id: "agents", label: "Agents", icon: "🤖" },
  { id: "connections", label: "Connections", icon: "🔌" },
  { id: "developer", label: "Developer", icon: "🛠" },
  { id: "browser", label: "Browser", icon: "🌐" },
  { id: "proxy", label: "Proxy Center", icon: "🔀" },
  { id: "camera", label: "Camera", icon: "📷" },
  { id: "voice", label: "Voice", icon: "🎙" },
  { id: "power", label: "Power", icon: "⚡" },
  { id: "settings", label: "Settings", icon: "⚙" },
  { id: "about", label: "About", icon: "ℹ" },
];

export default function Sidebar({
  page,
  onNavigate,
  collapsed,
  onToggle,
}: {
  page: PageId;
  onNavigate: (id: PageId) => void;
  collapsed: boolean;
  onToggle: () => void;
}) {
  return (
    <nav className={`sidebar${collapsed ? " collapsed" : ""}`} aria-label="Main navigation">
      <div className="sidebar-brand">
        <img src="/logo.svg" alt="CAMEO logo" />
        <div>
          <div className="brand-name">CAMEO</div>
          <div className="brand-sub">Autonomous AI</div>
        </div>
      </div>
      {NAV_ITEMS.map((item) => (
        <button
          key={item.id}
          className={`nav-item${page === item.id ? " active" : ""}`}
          onClick={() => onNavigate(item.id)}
          title={collapsed ? item.label : undefined}
          aria-current={page === item.id ? "page" : undefined}
        >
          <span className="nav-icon" aria-hidden="true">{item.icon}</span>
          <span className="nav-label">{item.label}</span>
        </button>
      ))}
      <div className="sidebar-spacer" />
      <button
        className="nav-item"
        onClick={onToggle}
        title={collapsed ? "Expand sidebar" : "Collapse sidebar"}
      >
        <span className="nav-icon" aria-hidden="true">{collapsed ? "»" : "«"}</span>
        <span className="nav-label">Collapse</span>
      </button>
    </nav>
  );
}
