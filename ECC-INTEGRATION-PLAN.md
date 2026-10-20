# CAMEO × ECC integration: audit and staged plan

**Audit date:** 2026-10-20
**ECC inspected:** `https://github.com/affaan-m/ECC`, shallow clone of `main` at commit `6f1881aeafa3c48f7b603cc4be4b2e4e70daad59` ("refactor(lib): split nasiko install, uninstall and tar extraction", committed 2026-10-19), package `ecc-universal` **v2.2.3**, **MIT license** (Copyright (c) 2026 Affaan Mustafa). 4,627 tracked files: 613 skills files, 71 agent definitions, 97 commands, 122 rules files, 9 hooks files, 503 test files, plus manifests, schemas, mcp-configs, install scripts (`install.sh`, `install.ps1`) and a Node/Python runtime under `src/` and `scripts/`.

## What ECC actually is

ECC is a MIT-licensed "agent harness operating system": Markdown guidance (skills/rules/agents), hooks, slash commands, portable manifests/schemas, a memory-vault convention, and an npm runtime (`ecc`, `ecc-install`, `ecc-memory-mcp`, dashboards). It targets many agent harnesses (Claude Code, Codex, Cursor, Gemini, OpenCode, ...). Its README warns to install only from verified channels and states the OSS repo stays MIT. Its capability is delivered through the *host harness* (e.g. Claude Code) reading that guidance; ECC itself is not a provider-neutral library embeddable in a Tauri/Rust app.

**Isolation used for this audit:** shallow clone into the OS temp directory; `node_modules` never created; no install script (`install.sh`, `install.ps1`, `scripts/*`), hook, MCP config, or bundle was executed; no network service from the repo was contacted.

## Compatibility classification

| Class | ECC components | CAMEO decision |
| --- | --- | --- |
| Reusable directly | None verbatim. Everything is either Markdown guidance for a JS/CLI agent harness or executable Node/Python scripts tied to that harness. | Nothing was copied verbatim into CAMEO. |
| Adaptable through a CAMEO adapter (implemented now) | Skill/registry concept (`manifests/context-packs/skill-registry@1.json`), phase workflow (`plan → implement → review → verify → remember`), verification-loop phase list, memory-vault idea + `schemas/memory.schema.json` kind/scope/status model | Bundled static capability registry `cameo/src/data/capabilities.json` + typed selection service; memory stored in CAMEO's own SQLite with inspect/edit/export/disable/delete controls. Registry metadata is our own text that *references* ECC skills under MIT with attribution; no ECC scripts ship or run. |
| Useful as design reference only | Agents' role prompts (e.g. `agents/build-error-resolver.md`, `agents/code-reviewer.md`), rules per language, hooks (SessionStart context, write-time formatting), plan-canvas, orchestrate/status tmux patterns, `council`/semi-independent multi-model review | Kept as design references recorded here. Providers remain CAMEO's OpenRouter/NVIDIA/Ollama/compatible layer; the five CAMEO identities stay; hooks/MCP are not enabled. |
| Incompatible / unsafe here | npm runtime (`scripts/ecc.js`, control pane, memory MCP) requiring global installs and PATH binaries; hooks system expecting Claude Code/Codex hook host; install scripts; councils and scheduled loops needing external CLIs; tmux/worktree orchestration; continuous-learning post-Hook writes | Not installed and not bundled. None of these may auto-enable MCP servers or execute repository instructions as commands. |

## Selected capabilities and why they fit

Priorities from the task: TDD, code review, security review, build-error repair, planning, documentation lookup, verification loops, refactoring/quality, frontend accessibility, git workflows, context management/learning. Of these, CAMEO already owns: bounded project inspection, provider chat, allowlisted checks (`npm test`, `npm run typecheck`, `npm run build`, `cargo test`, `cargo check`, `python -m pytest`, `go test`), per-file approved proposals, read-only three-agent review, and secret redaction. The registry adapts ECC guidance onto those existing tools instead of granting new ones:

- **Agents mapping (registry entry → CAMEO agent/tool surface):** [see `cameo/src/data/capabilities.json`]
  - `tdd-workflow` → FORGE: choose a failing test first, smallest change to pass it, then refactor with the check suite. Uses provider chat + allowlisted checks + per-file proposals only.
  - `verification-loop` → FORGE/verify phase: the six-phase idea is enforced by CAMEO's existing allowlisted check before completion - and completion is *never* claimed without real exit codes.
  - `build-error-repair` → FORGE/fix phase: uses the real failing command output; repairs go through the same per-file approval path.
  - `code-review`, `security-review` → ATLAS / SENTINEL via the existing read-only review workers (no shell/write access).
  - `plan-acceptance-criteria` → ATLAS planning phase: 3-6 concrete steps with acceptance criteria.
  - `documentation-lookup` → ATLAS research guidance: prefer pinned, cited sources (CAMEO has web research only through the user, not autonomous fetch).
  - `accessibility-review`, `conventional-commit-review` → PIXEL / reporting, marked `advisory-only` because PIXEL has no visual-testing tool and agent git push is prohibited.
  - `memory-governance` → CAMEO coordinator memory store described below.
- **Selection behavior:** `selectCapabilitiesFor(agent, phase)` returns only *active* entries matching the requesting agent and phase; `capabilityPhasesForStatus` also resolves declared dependencies (e.g. build-error-repair pulls in verification-loop) without duplicates. Unrelated skills are never loaded into one request. A corrupt registry raises instead of silently degrading guidance.
- **Real workflow wiring:** planning records a genuine `capabilities_applied` task event listing the selected capability ids and injects the compact gate text into the specialist system prompt. Review runs stay read-only with their existing 2-worker cap, 120 s deadline, 2 attempts, and SQLite persistence.

## Project memory (approved knowledge)

Inspired by ECC's unified-memory vault + `ecc.memory.v1` schema (MIT): CAMEO implements a **project-scoped** memory table `memory_entries` in its own SQLite database with kinds `decision | fact | lesson | runbook | preference | note | unresolved-issue`, scope fixed to `project`, and status `active | superseded`.

- Backend: `memory_add`, `memory_list`, `memory_update`, `memory_delete` Tauri commands with validation (known kinds, non-blank title ≤ 200 chars, body ≤ 20 000 chars). Nothing is sent to any provider.
- Secret handling: frontend `sanitizeMemoryText()` redacts credential-like values (same engine as review egress) before persistence; Rust also trims and size-bounds. Users are warned not to store secrets.
- UI (Developer page): add with kind/title/body, inline **Edit**, **Disable/Enable** (status flip, "disable" control), **Delete**, **Export JSON** download, and a Refresh list. Superseded entries stay inspectable but are excluded from "active" counts.
- Learning promotion: memory is user-authored/edited here; automatic learned-pattern promotion (ECC instincts/continuous-learning) is **not** implemented — the existing review flow proposes findings, and only the user's explicit memory edits persist knowledge. No recursive self-modification loop exists.

## Provider/model layer

Unchanged and provider-independent: OpenRouter, NVIDIA NIM, Ollama (local), and a configurable OpenAI-compatible endpoint. Credentials remain in the Windows OS credential store via the Rust backend. Reviewers can use the review-model selection pinned per review run. No claims are made about untested provider capabilities, and no private model artifacts or hidden chains of thought are involved.

## Security decisions

- Every manifest extension is *metadata*: enabling a capability grants only tools CAMEO already exposes (`provider_chat`, `bounded_project_snapshot`, `allowlisted_checks`, `file_proposals`, `review_workers`, `developer_probes`, `memory_store`). There is no "run script because a skill says so" path, no prompt-injection-driven permission grant, no MCP auto-enable, no session-start auto-context.
- File writes continue to require the per-file explicit approval with stale-preview protection; completion requires a real allowlisted check exit code 0; review remains non-executing with cancellation; untrusted repository content stays untrusted (explicit prompt boundaries).

## Files added/changed in this stage

- Added: `cameo/src/data/capabilities.json`, `cameo/src/services/capabilities.ts`, `cameo/src/services/memory.ts`, `cameo/tests/capabilities.spec.ts`, `cameo/tests/memory-service.spec.ts`, this document.
- Changed: `cameo/src/components/TaskExecutionWindow.tsx` (registry gates + `capabilities_applied` event), `cameo/src/pages/Developer.tsx` (registry card + memory controls), `cameo/src-tauri/src/commands/store.rs` (memory table + commands + tests), `cameo/src-tauri/src/lib.rs` (command registration), `cameo/tests/task-execution.spec.tsx` (gate/event assertions), `cameo/README.md`.

## Rollback

- Registry: delete `capabilities.json` usage from `TaskExecutionWindow.tsx`/`Developer.tsx`; nothing else depends on it at runtime and the primary task/approval flow is untouched.
- Memory: the extra SQLite table is additive; existing schemas unaffected. Remove the four commands from `lib.rs` to disable.
- Rollback points exist per prior stages: review-adapter changes remain isolated behind the task review panel; no upstream executables, dependencies, or agents were installed.

## Attribution

Capability metadata in `cameo/src/data/capabilities.json` is original text prepared for CAMEO that references MIT-licensed ECC components (`skills/tdd-workflow`, `skills/verification-loop`, `agents/build-error-resolver`, `rules/common/code-review`, `skills/security-review`, `commands/feature-development`, `skills/documentation-lookup`, `skills/accessibility`, `skills/unified-memory`, `schemas/memory.schema.json`), from `github.com/affaan-m/ECC`, `ecc-universal` 2.2.3, commit `6f1881aeafa3c48f7b603cc4be4b2e4e70daad59`, © 2026 Affaan Mustafa, MIT. If substantial ECC text is ever copied into CAMEO, the MIT notice above must accompany it.

## Remaining gated work (not implemented)

1. Language-specific rules loading (ECC rules/*) — deferred until a project-type detector exists in task planning.
2. Hooks-style automatic context/context-budget management — rejected for now (would need new privilege surface).
3. Session summaries/handoffs across app instances — partially covered by SQLite task persistence; a handoff format is future work.
4. Automatic learned-pattern promotion ("instincts") — deliberately excluded; requires an explicit human gate.
5. Worktree / tmux-style isolated coding workers — unchanged from previous deferred stages.
6. Live provider-side validation of the memory/registry UI was not possible in this environment (no launched Tauri session); coverage is through Vitest with mocked native boundaries and Rust unit tests.

## Verification record (Stage F) — 2026-10-20

- `npm run typecheck` (tsc --noEmit): passed, exit 0.
- `npm test` (vitest run): passed — 11 test files, 50 tests, exit 0 (capabilities registry ×6, memory service ×5, task-execution incl. capability gates/events ×4, and all pre-existing suites).
- `npm run build` (tsc && vite build): passed, exit 0; Vite reports pre-existing ineffective dynamic-import chunking warnings only. Output: `dist/` (index + window + main bundles, 49 modules transformed).
- `cargo test --manifest-path src-tauri/Cargo.toml`: passed — 9 Rust tests (added: memory schema/visibility and input validation under the real schema), exit 0.
- `cargo check --manifest-path src-tauri/Cargo.toml`: passed, exit 0, no warnings after wiring `MEMORY_SCHEMA_SQL` into `open_db`.
- `npm audit --omit=dev --audit-level=low`: “found 0 vulnerabilities”, exit 0.
- There is no Git metadata in the CAMEO folder, so a diff-based review is not possible; review was file-by-file.
- A bundle-free Windows release build (`npm run tauri build --no-bundle --ci`, fresh `CARGO_TARGET_DIR`) **passed**: exit 0, `Finished \`release\` profile [optimized] target(s) in 17m 44s`, built `cameo.exe` (8,446,464 bytes). Bundling (MSI/NSIS) was skipped: `--no-bundle` because no Windows packaging tools (WiX candle/light, NSIS) are installed in this environment — the earlier installed-app artifacts (`bundle/nsis/...setup.exe`, `bundle/msi/...msi`) under `src-tauri/target` predate this change. A first foreground attempt that reused the in-place target dir failed with os error 5 (locked `cameo.exe`), hence the fresh target directory.
- Runtime provider validation (real OpenRouter/NVIDIA/Ollama requests) was not performed; no credentials are available in this environment and none were requested.
