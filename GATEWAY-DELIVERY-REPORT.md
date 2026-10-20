# CAMEO Universal Secure Authentication Gateway — Delivery Report

## Architecture audit (existing code, audited before changes)
- Tauri 2 + React 19 + Vite desktop app; floating dock/panel/full window modes preserved.
- Backend `src-tauri/src/commands/`: `store.rs` (SQLite prefs/tasks/agent runs), `ai.rs` (provider chat + keyring keys), `window.rs`, `power.rs`, `browser.rs`, `devtools.rs` — untouched in behavior.
- Five agents (CAMEO/ATLAS/FORGE/PIXEL/SENTINEL) coordinate via `orchestration.ts` + task/agent-run tables; all agent chat flows through the single backend `chat` command — one permission surface by construction.
- Added without changing architecture: `gateway.rs` (auth gateway), `mcp.rs` (MCP connection manager), frontend `Connections` page.

## Components implemented (all in Rust backend, never the frontend)
| Component | Location |
|---|---|
| AuthManager / Revocation | `gateway.rs` — provider adapters, staged OAuth finish, revocation |
| CredentialVault | `gateway.rs` (per-provider/slot keyring entries) + `ai.rs` (model keys) |
| OAuthManager | `gateway.rs` — Authorization Code + PKCE (S256 enforced), CSRF state, loopback callback, Google token/refresh/revoke, GitHub device-less web flow |
| MCPConnectionManager | `mcp.rs` — stdio JSON-RPC lifecycle + streamable-HTTP remote support |
| PermissionEngine | `gateway.rs` — read/prepare/write/sensitive, per-provider, JIT approval with exact-action preview |
| AuditLogger | `gateway.rs` `audit_db` — plaintext-safe detail via `redact`/`redact_value` |
| ConnectionHealthMonitor | `gw_test_connection` — real authenticated read-only probes |
| BrowserAuthBridge | official authorization pages opened in the user's browser; loopback receiver with CSRF state validation |

## Authentication method per provider
- **Google & Gmail** — OAuth code flow + PKCE, scopes requested per enabled feature set (default `gmail.readonly`); client ID/secret stored per-slot; server-side token revocation on disconnect.
- **GitHub** — OAuth code flow, scopes `repo:status`, `read:user`; write tooling uses branches/PRs; destructive operations require approval.
- **Render** — API key only; validated with a read-only `render_list_services` call; stored exclusively in the OS credential store.
- **Remote MCP** — SSRF-guarded HTTPS; OAuth is *detected* (RFC 9728 protected-resource metadata + WWW-Authenticate) and reported, never auto-started (current MCP authorization spec).
- **NVIDIA NIM / OpenRouter / compatible / Ollama (model providers)** — API keys in Windows Credential Manager (`api_key.cameo_provider_*`), used only in the trusted backend.

## Exact permission scopes
- Google: `https://www.googleapis.com/auth/gmail.readonly` (extendable features gated in UI, never stored as one universal token).
- GitHub: `repo:status`, `read:user`.
- Permission levels default everything to read-only on first connect.

## Files changed/added
- `src-tauri/src/commands/gateway.rs` (new) — gateway commands + PKCE/OAuth/permissions/tools/audit.
- `src-tauri/src/commands/mcp.rs` (new) — MCP manager: add/list/update/remove/enable/validate/inspect/call, SSRF guard, stdio sandboxing.
- `src-tauri/src/commands/ai.rs` (edited) — NVIDIA 404 "Not found for account" now explained as tier limitation with a verified-working model hint.
- `src-tauri/src/lib.rs` (edited) — gateway + MCP schemas and 25 command registrations.
- `src/types/gateway.ts`, `src/services/gateway.ts`, `src/pages/Connections.tsx`, `src/styles/connections.css` (new) — Connections & Permissions page.
- `src/components/Sidebar.tsx`, `src/App.tsx`, `src/styles/global.css` (edited) — page wiring.
- `tests/connections.spec.tsx` (new).
- `scripts/verify_nvidia.ps1` (diagnostic; verifies the stored NVIDIA key without printing it).

## Security controls implemented
- OS-backed secret store (Windows Credential Manager); secrets never in frontend code, prompts, config, or logs.
- Redaction of secret-looking strings in logs and audit details.
- SSRF guards (HTTPS-only, host resolution checked against private ranges) for remote MCP.
- Endpoint allowlists + dangerous-header blocking for remote MCP (`User-Agent`, `Accept`, `Accept-Language`, `X-Session-Id`, `X-Organization-Id` only).
- PKCE S256 required; issuer and state validated; loopback only; user cancellation handled.
- Per-provider credential slots — cross-provider token reuse is impossible by construction.
- Global Stop control blocks connector/MCP calls and kills stdio children.
- stdio MCP: executable blocklist (cmd/powershell/bash/sh/wsl…), whitelisted Windows env, secrets only via child environment.
- MCP tool schemas validated before advertising; malformed schemas flagged, never silently trusted.

## Tests executed (actual results)
- `cargo test --lib` — **37 passed, 0 failed** (gateway: PKCE, permissions/approval enforcement, registry; mcp: auth discovery, tool extraction, vault isolation; existing store tests).
- `cargo build` (debug) — success; app launched via `tauri dev`, cameo.exe ran without panics.
- `tsc --noEmit` — clean. `vitest` — **55 passed / 0 failed** (incl. new Connections suite). `vite build` — success.
- Live provider checks: NVIDIA key verified against `integrate.api.nvidia.com` (81 models served; real chat completion `openai/gpt-oss-20b` returned content). OpenRouter public catalog serves 468 models.
- All mocking for tests provisions mock providers; no email, repos, or deployments were touched.

## Setup steps for you
1. **NVIDIA (done)** — key already stored; it works. For chat, pick a model your tier serves (e.g., `openai/gpt-oss-20b`). Others 404 "Not found for account" — that is NVIDIA's tier gate.
2. **OpenRouter** — Settings → save your OpenRouter key (`sk-or-…`).
3. **Google/Gmail** — create an OAuth client at console.cloud.google.com, add `http://127.0.0.1:<port>` to authorized redirects, press “Save client ID”, then Connect. Toolbar revoke: https://myaccount.google.com/connections.
4. **GitHub** — create an OAuth app per github.com/settings/applications, save client ID, Connect. Revoke there, then Disconnect in CAMEO.
5. **Render** — Connections → Render card → paste key → Save & test. Revoke via Team Settings → API Keys.
6. **MCP servers** — Connections → MCP Server Manager → add (stdio or HTTPS), review executable/args/cwd, then Enable.

## Known limitations
- Google token refresh works only while the client secret is stored; delete revokes local secrets only (Google-side revoke needed).
- Remote MCP OAuth is detected and surfaced but cannot be completed in-app yet (no dynamic client registration UI) — matches the “detect, don’t assume” requirement.
- GateGuard-equivalent agent-level enforcement demands connectors be used through `gw_connector_call`/`mcp_call_tool`; shell/browser/devtools paths belong to their existing approval flows.
- Browser automation for sign-in is approved-task only by policy; cookie/session extraction is not implemented by design.

## Release build verification (Windows)
- `cargo build --release` and `npm run tauri build` both pass (exit 0, zero errors).
- Artifacts: `src-tauri/target/release/cameo.exe` (8.7 MB), MSI `CAMEO AI_0.1.0_x64_en-US.msi` (4.5 MB), NSIS `CAMEO AI_0.1.0_x64-setup.exe` (3.1 MB).
- Fix applied: removed `cdylib` from `crate-type` in `src-tauri/Cargo.toml` — with `panic = "abort"` + fat LTO on MSVC, the `cameo_lib.dll` link step failed with an opaque `link.exe` error. The desktop app only uses the bin + rlib; the cdylib was unused.
