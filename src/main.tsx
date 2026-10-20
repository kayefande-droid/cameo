import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles/global.css";

// Restore saved theme before first paint.
if (typeof window !== "undefined") {
  const saved = localStorage.getItem("cameo-theme");
  if (saved === "light" || saved === "dark") {
    document.documentElement.dataset.theme = saved;
  }
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
