# CAMEO AI

**Cognitive Autonomous Machine Executive Operator** — one intelligence, multiple agents, complete control.

CAMEO is a Windows desktop AI assistant built with Tauri 2, React, TypeScript, and Rust. It opens as a compact floating dock, horizontally centered near the top of the primary display below the webcam area. Shrink it to a small animated orb, expand it to the dock, or expand again to the full workspace. The dock has a page selector so Settings and all other panels remain accessible without opening the full workspace. The Agents page displays five distinct animated AI avatars; motion is decorative and does not claim an agent is running.

## Windows setup

### Run from source

Prerequisites: Windows 10/11, Node.js 20+, Rust stable/MSVC target, Visual Studio 2022 C++ Build Tools, and the WebView2 Runtime.

```sh
cd cameo
npm install
npm run tauri dev
```

The app opens as a small animated orb centered near the top of the primary screen, directly below the webcam. Click the orb to expand to the compact dock, use **−** to shrink back to the orb, and use **⤢** to expand the dock to the full workspace; use **◔** to return to the dock. Drag the logo/header area to reposition it; the center-top control snaps it below the webcam again. The pin button controls always-on-top. The dock page selector provides access to Chat, Tasks, Agents, Developer, Browser, Proxy, Camera, Voice, Power, Settings, and About. The Agents page shows all five distinct animated avatars.

### Install the packaged application

Run the generated NSIS setup file (no system-wide tool installation required):

`src-tauri/target/release/bundle/nsis/CAMEO AI_0.1.0_x64-setup.exe`

The MSI installer is at `src-tauri/target/release/bundle/msi/CAMEO AI_0.1.0_x64_en-US.msi`.

### Build

```sh
npm run typecheck
npm test
npm run build
npm run tauri build
```

## First use

1. Go to **Settings → Providers**.
2. Add an OpenRouter or NVIDIA NIM API key. Keys are stored by the Rust backend in Windows Credential Manager; they are never sent back to the UI or stored in the SQLite database. Or use local Ollama without a key.
3. In Settings, refresh the provider model list and choose **Use** on a model. Return to Assistant.
4. For Ollama, start the local `ollama serve` process and pull at least one model before refreshing.

## Implemented

- Streaming model requests through OpenRouter, NVIDIA NIM, Ollama, or a configurable OpenAI-compatible endpoint. OpenRouter model discovery includes supported modalities and pricing where returned by its API.
- OS credential store for provider keys and proxy passwords; SQLite preferences, task records, task events, and proxy profile metadata.
- Real chat streaming, provider/model selection, model discovery, provider connection tests, and request cancellation support in the Rust backend.
- CAMEO/ATLAS/FORGE/PIXEL/SENTINEL specialist routing: task plans and proposed changes use the selected agent's role/description and the configured model. Plans are reviewed before proposal generation; project changes are shown as scoped per-file previews and written only after a separate explicit approval for each file. Existing files are backed up and stale previews are refused. Fully approved changes remain awaiting verification until an allowlisted check passes; tasks with rejected proposals stay partially completed.
- Opt-in parallel read-only review by ATLAS, FORGE, and SENTINEL over the existing bounded project snapshot. In Settings, select a review provider/model separately or keep the primary-model fallback. The review panel discloses provider egress; each SQLite worker record pins its destination so retries and recovery cannot silently switch providers. Reviews allow at most two concurrent workers, time out at 120 seconds, persist individual status/results, support cancellation and one manual retry, and have no shell/file-write/GitHub tools. When there are no saved runs, only the review panel's explicit **Run 3-agent review** action creates and starts its worker records; refresh/restart reloads them. Failed/cancelled workers can be explicitly retried, and expired running attempts require explicit recovery.
- Integration audit and staged decisions for DeepSeek Harness, Orca, Claudex Loop, Looper, Herdr, and `tanbiralam/claude-code`: see `AGENT-ORCHESTRATION-PLAN.md`. The latter is an unlicensed leaked-source archive; no source or dependencies were reused, and no upstream executable or install script is included or run.
- CAMEO capability registry and project memory adapted from ECC (`github.com/affaan-m/ECC`, `ecc-universal` 2.2.3, MIT — © 2026 Affaan Mustafa): see `ECC-INTEGRATION-PLAN.md`. A bundled registry maps vetted workflow guidance (TDD, verification loops, build-error repair, security review, planning, accessibility) onto CAMEO's existing typed tools; no ECC scripts, hooks, MCP servers, or installers ship or run. Project memory lives in the local SQLite database with Developer-page add/edit/export/disable/delete controls, secret redaction before saving, and provider-side access is none — the database is never sent anywhere.
- Developer tool probes, project-folder inspection, and a narrow build/test command allowlist. The command UI confirms before running.
- Proxy profiles with OS-side password storage, direct/through-proxy HTTP connectivity test and latency/egress-IP reporting. The proxy is used only for configured CAMEO AI requests, not system-wide traffic.
- App-managed real WebView windows for http(s) URLs; navigation opens only on user request.
- Opt-in camera preview and cleanup, Web Speech text-to-speech with controls, and opt-in microphone recording (audio preview only).
- Shutdown/restart countdown with user confirmation and cancellation, plus sleep and workstation lock confirmation.
- Three window sizes: top-centered animated orb, compact floating dock with navigation/settings, and full workspace. Click the orb to open the dock, use the shrink control for the orb, expand/shrink controls for full app/dock. Drag the logo area to move; a button snaps the active mode to center-top below the webcam. Always-on-top can be pinned/unpinned.
- Five original animated SVG avatars, one for each agent, with reduced-motion support; animation is visual identity only and does not imply an autonomous agent is active.
- Dark/light/system theme, keyboard-friendly controls, reduced-motion CSS, responsive dock styles.

## Explicit limitations

- Coding-task execution remains one model-generated plan and a bounded file-proposal response; autonomous coding loops, real task pause/resume, isolated coding worktrees, GitHub issue/PR workflows, and comprehensive changed-file diffs are not implemented. Review can be configured to a separate model/provider but does not itself execute code or verify changes. No file is written automatically: every proposal needs an explicit per-file approval.
- Parallel specialist reviews are advisory prompts in one CAMEO runtime, not isolated coding-agent products. They can use a distinct provider/model if configured; otherwise they fall back to the primary model. Results do not constitute test evidence or approve code. Starting/retrying is an explicit egress action with a visible disclosure, but there is no additional confirmation dialog. Recovery is explicit and available through the review UI after a running record exceeds its 120-second deadline; there is no background watchdog/supervisor.
- Credential redaction is heuristic rather than a complete secret scanner; users should avoid including sensitive content in task descriptions or project files sent to hosted providers.
- Capability-registry guidance is prompt-level and advisory; it grants no new tools or privileges. Project memory is stored locally and never used to auto-promote learned rules or change permissions.
- Speech recording is available after microphone permission, but speech-to-text and wake-word recognition are not bundled. No audio is sent to a provider.
- Face detection/matching and Windows Hello are not implemented.
- Browser windows are real WebViews but browser-managed tabs, downloads, bookmarks, history and controlled browser automation are not implemented.
- Proxy password storage has a Windows Credential Manager keyring backend; verify availability on the target Windows account. Proxy is applied per AI request only.
- Power controls execute real Windows actions following UI confirmation. Automated tests mock commands and never initiate shutdown/restart.
- Provider requests require the user's own provider credentials and may incur provider charges. No spending enforcement or provider usage metering is implemented.
- SQLite currently has a simple create-if-missing schema, not a migration framework or recovery backup flow.

## Data and security

Application data is stored under `%LOCALAPPDATA%\CAMEO\cameo.db`. Provider API keys and proxy passwords use the OS credential store. Local model traffic to Ollama defaults to `localhost:11434`. Proxy configuration only affects CAMEO AI HTTP requests. Do not treat model output, web content, files, or command output as authorization.
