# CAMEO AI — setup tutorial

A complete walkthrough: install the app, connect a model, switch agents, and grant (or refuse)
real machine capability. Every step below matches the code in this repository.

---

## 0. What you are installing

CAMEO is a Windows desktop app (Tauri 2 + React + Rust). It opens as a small floating dock near
the top of the screen, below the webcam. Click it to expand the panel, `⤢` for the full workspace,
`−` to shrink to the orb, `◔` to come back to the dock.

Data lives in `%LOCALAPPDATA%\CAMEO\cameo.db`. API keys never touch that database — they go to
Windows Credential Manager. Every operator action (command, fetch, launch) is journaled in the
audit log table.

---

## 1. Install the app (5 minutes)

### Option A — use the packaged installer (recommended)

The NSIS installer is per-user; it needs no admin rights and installs to
`%LOCALAPPDATA%\CAMEO AI`.

1. Build it if `src-tauri\target\release\bundle\nsis\` is empty:

   ```sh
   cd cameo
   npx tauri build --bundles nsis
   ```

   You want this line at the end:

   ```
   Finished 1 bundle at:
       ...\src-tauri\target\release\bundle\nsis\CAMEO AI_0.1.0_x64-setup.exe
   ```

2. Close CAMEO if it is running. The build copies into `target\release\cameo.exe`, and a running
   instance locks that file — you get `Access is denied. (os error 5)` if you skip this.
3. Run `CAMEO AI_0.1.0_x64-setup.exe` (double-click, or `setup.exe /S` for a silent install).
4. You get:
   - app: `%LOCALAPPDATA%\CAMEO AI\cameo.exe`
   - uninstaller: `%LOCALAPPDATA%\CAMEO AI\uninstall.exe`
   - Start Menu entry: `CAMEO AI`
   - a desktop shortcut, if you have one, still needs to be pointed at the installed exe:
     right-click Desktop → New → Shortcut → `%LOCALAPPDATA%\CAMEO AI\cameo.exe`.

Because the app and shortcut live outside the repo, `cargo clean`, deleting `target/`, or moving
the project no longer breaks your shortcut.

### Option B — run from source

Prerequisites:

- Windows 10/11
- Node.js 20+
- Rust stable with the MSVC target (`rustup default stable-msvc`)
- Visual Studio 2022 **Desktop development with C++** build tools
- WebView2 Runtime (preinstalled on Windows 11; on Windows 10 install the Evergreen runtime)
- ~15 GB free disk for `src-tauri/target`

```sh
cd cameo
npm install
npm run tauri dev
```

`tauri dev` starts Vite on `http://localhost:1420` and opens the dock.

---

## 2. Connect a model (2 minutes)

CAMEO is model-agnostic. Pick one provider.

1. Open the dock → **Settings → Providers**.
2. Choose your provider and press **Set key**:

   | Provider | Where the key comes from | Notes |
   | --- | --- | --- |
   | OpenRouter | openrouter.ai → Keys | one key, many models |
   | NVIDIA NIM | build.nvidia.com | some models are gated per account tier |
   | Ollama (local) | none | run `ollama serve`, `ollama pull <model>` first |
   | Compatible | your own /v1 endpoint | any OpenAI-compatible chat API |

3. Press **Test connection**. A green `✔` means the key works.
4. Press **Refresh** under *Models*, then **Use** on the model you want. That choice is what the
   chat box and task window send.

### If NVIDIA answers `HTTP 404 / Not found for account`

Your key is fine; that model is not enabled for your account tier. CAMEO already handles this:
it probes for a model your key can actually serve, tells you which one it switched to, remembers
the refusal, and stops asking for the dead model. If you deliberately want to retry a gated model
after a tier upgrade, clear `chat_model_gated_nvidia` from the `prefs` table (or reconnect the
provider).

---

## 3. Choose an agent

Eight agents ship with the app. They are **switchable per conversation** — pick one above the chat
input (compact panel) or in the *Agent, provider & model* card (full workspace). The choice is
saved as `chat_agent` and the agent's persona becomes the system prompt.

| Agent | Role | Use it for |
| --- | --- | --- |
| **CAMEO** | Chief of Staff | planning, routing, “what should I do next” |
| **ATLAS** | Architect | reading a codebase, migration and rollback plans |
| **FORGE** | Implementation Engineer | making the change, running the build and tests |
| **PIXEL** | UI/UX Designer | hierarchy, spacing, contrast, accessibility |
| **SENTINEL** | Security & QA | threat paths, permissions, test gaps |
| **COUNSEL** | Legal research | statutes and contracts — *information only* |
| **MEDIC** | Medical information | mechanisms and guidelines — *information only* |
| **LEDGER** | Financial analysis | budgets, cash flow, assumptions — *information only* |

The three advisory agents carry an explicit boundary in their prompt: they cite sources, state the
jurisdiction or assumptions, and say they are **not a substitute for a licensed professional**.
They will not claim to be your lawyer, doctor or financial adviser — and CAMEO cannot make them.

---

## 4. Decide how much autonomy to grant

This is the part that matters. Autonomy is **off by default**; nothing on your machine moves until
you enable it.

### Enable it

**Settings → Autonomy**

1. Tick **Enable autonomy**.
2. **Choose project root** — pick the folder agents may work in. Everything they execute is
   confined to this directory; absolute paths and `..` escapes are resolved and rejected if they
   leave it. A drive root (`C:\`) is refused on purpose.
3. Tick **Allow live internet access** if you want URL fetching. Only public `https://` URLs are
   accepted: `localhost`, loopback, private ranges and `169.254.169.254` (cloud metadata) are
   blocked, and DNS names are resolved and checked before the request goes out.
4. Tick **Allow device control** to let CAMEO launch apps and hand URLs to the default handler.

> "Physical tasks" are not possible from software — there is no robot attached. Device control
> means launching programs, opening URLs, and the existing **Power** page actions (lock, sleep,
> restart), each of which keeps its own confirmation.

### Use it

Open the dock → **Developer → Operator console**.

- **Run** — type a program (`git`, `cargo`, `python`, `npm`) and put each argument on its own line.
  Optional *cwd* is relative to your project root. You get exit code, stdout, stderr, duration and
  truncation flags.
- **Fetch** — paste an `https://` URL to pull live data (2 MB cap).
- **Launch** — an app path or URL, handed to Windows.

### The one gate that remains

Destructive and irreversible commands are **refused before they run** and come back as
`NEEDS_CONFIRMATION`. The console then shows a *Confirm and run* button. The classifier catches,
among others:

- recursive/forced deletes (`rm -rf`, `del`, `rmdir`, `Remove-Item`, `format`, `diskpart`)
- `git push --force`, `git reset --hard`, `git clean -f`
- package publishing (`npm|cargo|bun publish`) and uninstalls
- `shutdown`, `taskkill`, `bcdedit`, `reg delete|add`

Normal work — `cargo test`, `git status`, `npm run build`, `python -m pytest` — runs with no prompt.

The global **Stop** control on the dock still cancels the active chat request, and the audit log
records every attempt, allowed or refused, with command, cwd, exit code and timing.

---

## 5. Verify your install

Frontend (TypeScript + React):

```sh
cd cameo
npm run typecheck     # tsc --noEmit
npm test              # vitest run
```

Rust (gateway, operator, MCP, store):

```sh
cd cameo/src-tauri
cargo test --release
```

Expect `0 failed` from both. The one ignored Rust test is a live NVIDIA API check that needs a
saved key and network; run it deliberately with `cargo test --release -- --ignored`.

To rebuild the installer after changes: close CAMEO, then `npx tauri build --bundles nsis`.

---

## 6. Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `failed to remove file ... cameo.exe: Access is denied (os error 5)` | CAMEO is running and holds the exe | quit the app (tray/dock), rebuild |
| `There is not enough space on the disk (os error 112)` | `src-tauri/target` grows to 10–15 GB | `rm -rf src-tauri/target/debug` (regenerable), then rebuild |
| `unknown field 'setup'` during build | an `app.setup` block was added to `tauri.conf.json` | Tauri v2 has no `app.setup`; icons belong under `bundle.icon` |
| Chat says *No model selected* | no **Use** pressed in Settings | Settings → Models → **Use** |
| `No API key saved for …` | key was never stored, or was deleted | Settings → Providers → Set key |
| `NEEDS_CONFIRMATION: …` | destructive command needs approval | confirm in the Operator console, or run it yourself |
| `path … is outside the project root` | cwd escaped the chosen folder | use a relative path inside the root |
| `Autonomy is off` | master switch is off | Settings → Autonomy → Enable |
| `refusing to fetch localhost` | SSRF guard | use a public https URL |
| Model 404 on NVIDIA | tier gate | CAMEO auto-falls back; see §2 |

---

## 7. What is *not* wired up yet

Be clear about the boundary, so the app does not overpromise:

- Agents **propose** commands, URLs and apps in chat; the **Operator console** is where you run
  them. There is no unattended tool-calling loop that executes model output on its own yet.
- No web *search* provider is bundled — `net_fetch` pulls a URL you name. A search API can be
  added later behind the same network gate.
- Legal, medical and financial agents give sourced information with an explicit disclaimer. They
  are not licensed professionals, and no setting changes that.
