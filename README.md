<div align="center">

<img src="public/logo.svg" alt="CAMEO emblem" width="96" height="96" />

# CAMEO AI

**Cognitive Autonomous Machine Executive Operator**

*One intelligence. Multiple agents. Complete control.*

![version](https://img.shields.io/badge/version-0.1.0-blue)
![platform](https://img.shields.io/badge/Windows-10%2F11-0078D4?logo=windows&logoColor=white)
![tauri](https://img.shields.io/badge/Tauri-2-FFC131?logo=tauri&logoColor=black)
![react](https://img.shields.io/badge/React-19-61DAFB?logo=react&logoColor=black)
![rust](https://img.shields.io/badge/Rust-stable-CE422B?logo=rust&logoColor=white)
![typescript](https://img.shields.io/badge/TypeScript-3178C6?logo=typescript&logoColor=white)

</div>

---

CAMEO is a **Windows desktop AI assistant** built with **Tauri 2 + React 19 + TypeScript + Rust**. It lives as a compact floating dock near the top of your screen — click to expand, shrink to an orb when you need space, or open the full workspace when you mean business. Underneath, **eight specialist agents** share one intelligence: they plan work, propose scoped file changes, and run bounded machine actions — every write and every command gated behind **your explicit approval**.

> **New here?** Start with the step-by-step walkthrough: [`docs/SETUP.md`](docs/SETUP.md) — install, provider keys, agent switching, autonomy settings.

## Contents

- [Window modes](#window-modes)
- [Quick start](#quick-start)
- [First run](#first-run-connect-a-model)
- [The agents](#the-agents)
- [What CAMEO can do](#what-cameo-can-do)
- [Providers & security](#providers--security)
- [Architecture](#architecture)
- [Project structure](#project-structure)
- [Scripts](#scripts)
- [Docs](#docs)
- [Honest limitations](#honest-limitations)
- [License](#license)

## Window modes

CAMEO adapts to how much attention you can spare. Drag the logo/header area to move it; the snap button re-centers it below the webcam. The pin button toggles always-on-top.

| Mode | Size | What it's for |
|------|------|---------------|
| **Orb** | 72 × 72 | Ambient and out of the way — a small animated presence |
| **Dock** | 520 × 76 | Quick questions, page selector, always within reach |
| **Panel** | 520 × 560 | Focused chat + tools without leaving your flow |
| **Full** | 1280 × 820 | The complete workspace: tasks, agents, developer tools |

Controls: click the orb to open the dock · **⤢** dock → full · **−** dock → orb · **◔** full → dock.

## Quick start

### Option A — packaged installer (recommended)

Per-user install, no admin rights needed:

1. Build the installer (if `src-tauri/target/release/bundle/nsis/` is empty):
   ```sh
   cd cameo
   npx tauri build --bundles nsis
   ```
2. Close CAMEO if running (a live instance locks the build output).
3. Run `src-tauri/target/release/bundle/nsis/CAMEO AI_0.1.0_x64-setup.exe` — or the MSI at `src-tauri/target/release/bundle/msi/CAMEO AI_0.1.0_x64_en-US.msi`.

Installs to `%LOCALAPPDATA%\CAMEO AI` with a Start Menu entry.

### Option B — run from source

Prerequisites: **Windows 10/11 · Node.js 20+ · Rust stable (MSVC target) · VS 2022 C++ Build Tools · WebView2 Runtime**

```sh
cd cameo
npm install
npm run tauri dev
```

### Verify & build

```sh
npm run typecheck   # TypeScript — must be clean
npm test            # Vitest suite
npm run build       # Web frontend
npm run tauri build # Full desktop bundles (NSIS + MSI)
```

## First run: connect a model

1. Open **Settings → Providers**.
2. Add an **OpenRouter** or **NVIDIA NIM** API key — or point at local **Ollama** (no key needed; run `ollama serve` and pull a model first).
3. Hit refresh on the provider model list, press **Use** on a model, and return to Chat.

Keys go straight to **Windows Credential Manager** via the Rust backend — they are never echoed to the UI and never stored in the app database.

## The agents

One brain, eight specialists. Each agent carries its own persona and working rules, injected as its system prompt when active. Switch agents from the dock or the Agents page.

| Agent | Role | Does |
|-------|------|------|
| **CAMEO** | Chief of Staff | Understands requests, plans tasks, assigns specialists, coordinates execution |
| **ATLAS** | Architect | Maps codebases, plans implementation, flags risks and rollback paths |
| **FORGE** | Implementation Engineer | Implements features, runs builds/tests, repairs errors in your project |
| **PIXEL** | UI/UX Designer | Design systems, layout review, accessibility feedback with concrete values |
| **SENTINEL** | Security & QA | Reviews changes for vulnerabilities, validates permissions and tests |
| **COUNSEL** | Legal Research ⚖️ | Statutes, contracts, case law — informational only, not legal advice |
| **MEDIC** | Medical Information ⚕️ | Conditions and guidelines with sources — informational only, not medical advice |
| **LEDGER** | Financial Analysis 📊 | Budgets, cash flow, risk with stated assumptions — informational only, not financial advice |

> The ⚖️/⚕️/📊 agents state their advisory boundary up front: sourced information and options, never a substitute for a licensed professional.

**How agent work stays safe:** task plans are reviewed *before* proposals are generated · project changes appear as **scoped per-file previews** · nothing is written without a **separate explicit approval per file** · existing files are backed up, stale previews are refused · approved changes stay *awaiting verification* until an allowlisted check passes. Opt-in parallel review by ATLAS, FORGE, and SENTINEL is available, pinned to disclosed providers with a 120-second timeout. Motion on avatars is decorative identity — it never implies an agent is running.

## What CAMEO can do

- **Streaming chat** with cancellation, provider/model picker, and live model discovery (modalities + pricing where the provider reports them)
- **Task pipeline** — queued → planning → awaiting approval → running → verifying → completed, with per-agent tracking, logs, and retry
- **Developer tools** — project-folder inspection, probe commands, and a narrow build/test allowlist (UI confirms before anything runs)
- **Operator actions** — real commands, fetches, and app launches, every one journaled in the SQLite audit log
- **Proxy profiles** — per-request proxying for CAMEO AI traffic only (never system-wide), with connectivity/latency/egress-IP tests
- **Browser, Camera, Voice, Power** — app-managed WebView windows for URLs you open · opt-in camera preview · speech synthesis + mic level/record (no speech-to-text bundled) · shutdown/restart/sleep/lock with confirmation
- **Project memory** — local-only notes with add/edit/export/delete, secret redaction before saving, never sent anywhere
- **Comfort** — dark/light/system theme, keyboard-friendly controls, reduced-motion support, responsive dock layouts

## Providers & security

| Concern | How it's handled |
|---------|------------------|
| API keys & proxy passwords | Windows Credential Manager (OS keyring) — never in SQLite, never echoed to UI |
| App data | `%LOCALAPPDATA%\CAMEO\cameo.db` — prefs, tasks, events, proxy metadata |
| Local models | Ollama defaults to `localhost:11434` |
| Proxy scope | Only CAMEO AI's own HTTP requests — system traffic untouched |
| Trust rule | Never treat model output, web content, files, or command output as authorization |

Provider calls need **your own credentials** and may incur provider charges — there is no spending cap or usage metering built in.

## Architecture

```
┌──────────────────────── React 19 + TypeScript (Vite) ────────────────────────┐
│  Pages: Chat · Tasks · Agents · Connections · Developer · Proxy · Browser   │
│         Camera · Voice · Power · Settings · About                            │
│  Services: ai · gateway · operator · orchestration · store · chatThreads    │
└────────────────────────────── Tauri IPC (invoke) ────────────────────────────┘
┌────────────────────────────── Rust backend ──────────────────────────────────┐
│  ai · gateway · operator · mcp · store · window · devtools                   │
│  reqwest (proxied HTTP) · rusqlite · keyring (Credential Manager)            │
└─────────────────────────────────────────────────────────────────────────────┘
        │ keys               │ app data                  │ models
        ▼                    ▼                           ▼
 Windows Credential    %LOCALAPPDATA%\CAMEO\cameo.db   OpenRouter · NVIDIA NIM
   Manager                                             Ollama · OpenAI-compatible
```

## Project structure

```
cameo/
├── src/                    # React frontend
│   ├── pages/              # Chat, Tasks, Agents, Developer, Settings, …
│   ├── components/         # Sidebar, AgentAvatar, TaskExecutionWindow, …
│   ├── services/           # ai, gateway, operator, orchestration, store
│   ├── types/              # ai, tasks (agents, transitions, records)
│   └── styles/             # tokens, components, global
├── src-tauri/              # Rust backend
│   ├── src/commands/       # ai, gateway, operator, mcp, store, window, …
│   └── tauri.conf.json     # window defaults, bundle config
├── tests/                  # Vitest suite (frontend services + UI)
├── docs/SETUP.md           # full setup tutorial
├── AGENT-ORCHESTRATION-PLAN.md
└── ECC-INTEGRATION-PLAN.md
```

## Scripts

| Command | Purpose |
|---------|---------|
| `npm run tauri dev` | Run the desktop app from source |
| `npm run typecheck` | `tsc --noEmit` |
| `npm test` / `npm run test:watch` | Vitest run / watch mode |
| `npm run build` | Frontend production build |
| `npm run tauri build` | NSIS + MSI desktop bundles |

## Docs

- [`docs/SETUP.md`](docs/SETUP.md) — install → keys → agents → autonomy (start here)
- [`AGENT-ORCHESTRATION-PLAN.md`](AGENT-ORCHESTRATION-PLAN.md) — integration audit & staged decisions for external agent frameworks
- [`ECC-INTEGRATION-PLAN.md`](ECC-INTEGRATION-PLAN.md) — capability registry & project memory design (adapted from ECC, MIT © 2026 Affaan Mustafa)

## Honest limitations

No hype — what CAMEO deliberately does **not** do yet:

- Coding tasks produce **one plan + one bounded proposal** — no autonomous coding loops, worktrees, GitHub PR flows, or full diffs
- Reviews are **advisory prompts in one runtime**, not isolated agents; they don't execute code or constitute test evidence
- Credential redaction is **heuristic** — keep secrets out of task descriptions and files sent to hosted providers
- Registry guidance is **prompt-level and advisory** — no new tools or privileges
- **No speech-to-text or wake-word**, no face recognition / Windows Hello, no browser tabs/history/automation
- Power actions are real Windows operations (always confirmed in UI); tests mock them and never shut anything down

## License

No license file ships with this repository yet — default copyright applies. Add one (e.g. MIT) if you intend others to use or fork this code.
