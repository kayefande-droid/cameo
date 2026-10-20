const fs = require("fs");
const files = [
  "src/App.tsx",
  "src/components/Sidebar.tsx",
  "src/components/DeleteConfirmPanel.tsx",
  "src/components/TaskExecutionWindow.tsx",
  "src/pages/Agents.tsx",
  "src/pages/Browser.tsx",
  "src/pages/Camera.tsx",
  "src/pages/Chat.tsx",
  "src/pages/Developer.tsx",
  "src/pages/Proxy.tsx",
  "src/pages/Settings.tsx",
  "src/pages/Tasks.tsx",
  "src/pages/Voice.tsx",
  "src/pages/Power.tsx",
];
const re = /import React, \{([^}]*)\}\s*from "react";/;
for (const f of files) {
  let s = fs.readFileSync(f, "utf8");
  s = s.replace(re, (m, hooks) => `import {${hooks}} from "react";`);
  fs.writeFileSync(f, s);
}
console.log("done");
