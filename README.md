# Roundtable

Open-source desktop + Android app for chatting with AI models and running
multiple role-based agents that talk to each other.

**Targets:** Windows, Linux, Android (no macOS/iOS)
**Stack:** Tauri 2 (Rust core) · React + TypeScript + Vite · SQLite

## Repository layout

```
roundtable/
├── Cargo.toml                 # Rust workspace (desktop + shared core)
├── crates/
│   └── roundtable-core/       # Shared core — reused by desktop AND Android
├── src-tauri/                 # Desktop shell (Tauri 2)
│   ├── src/lib.rs             # command wiring, delegates to roundtable-core
│   ├── tauri.conf.json
│   └── capabilities/          # Tauri permission capabilities
├── src/                       # Frontend (React + TS, built by Vite)
│   ├── App.tsx                # 3-pane shell, palette, settings
│   ├── components/            # Sidebar, Conversation, Inspector, palette…
│   ├── lib/                   # theme, tauri invoke helpers
│   └── styles/                # tokens.css (design tokens), base, app
├── index.html                 # pre-paint theme script (no flash on load)
└── vite.config.ts
```

## Prerequisites

- Rust ≥ 1.90 (`rustup default stable`)
- Node ≥ 20
- **Linux:** WebKitGTK 4.1 + GTK 3 dev packages:
  `sudo apt install libwebkit2gtk-4.1-dev build-essential libssl-dev \
   libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev`
- **Windows:** MSVC build tools
- **Android builds:** JDK 17, Android SDK (platform-tools + a platform),
  NDK r27, and the Rust Android targets
  (`aarch64-linux-android` at minimum); run `npx tauri android init` once —
  see [docs/android.md](docs/android.md).

## Install (released builds)

- **Windows:** download the `.msi` (per-machine installer) or `-setup.exe`
  (NSIS) from the
  [latest release](https://github.com/yashu2733537/roundtable/releases/latest),
  run it, launch Roundtable from Start. Updates install in-app automatically.
- **Linux:** download the `.AppImage` (`chmod +x`, run), or install the
  `.deb` with `sudo dpkg -i roundtable_*.deb`.
- **Android:** download the signed `.apk`, allow install from the browser
  once, install. Cloud providers and remote mode only — local models are
  out of scope on phones. Prefer pairing with your desktop over Tailscale;
  see [docs/remote.md](docs/remote.md).

## Run (development)

```bash
npm install
npm run tauri dev      # starts Vite + compiles + opens the app window
```

The frontend alone (no Rust, so the About card will show its error state) is
available with `npm run dev` at http://localhost:1420.

## Build (release bundles)

```bash
npm run tauri build
# → src-tauri/target/release/bundle/
#   Linux: .deb / .rpm / .AppImage
#   Windows: .msi / .nsis-setup.exe
```

## Test

```bash
npm run build          # tsc --noEmit + vite build
cargo test -p roundtable-core
```

## Keyboard

| Shortcut | Action |
| --- | --- |
| `Ctrl+K` | Toggle command palette |
| `Esc` | Close palette / dialogs |

## Phase status

- [x] **P1** Foundation + single chat: Tauri 2 scaffold (Windows/Linux,
      `mobile_entry_point` ready for Android), provider layer
      (OpenAI-compatible `/models` + SSE `/chat/completions`, cancel,
      UTF-8-safe streaming), OS credential-store keys, SQLite persistence
      with migrations (providers, conversations, messages, settings), chat UI
      (new/rename/delete/search, streaming, Stop, regenerate, edit-and-resend,
      copy), model picker grouped by provider + manual id, settings
      (providers, theme, default model, JSON export/import, data folder),
      Ollama detection + guide, theme system + design tokens
- [x] **P2** Agents: agent entity (SQLite migration v3) with avatar, role,
      system prompt, provider/model, temperature, max tokens, enabled flag;
      builder UI with live preview, sliders, test mini-chat, duplicate,
      delete, JSON import/export; six built-in role templates (JSON);
      per-conversation agent binding with automatic system/model; agent
      switcher + @mention routing; agent avatars on messages; Inspector shows
      last run (model, tokens, latency, exact request); agents list with
      search + status/provider filters
- [x] **P3** Multi-agent rooms: room entity (SQLite migration v4) with member
      agents, mode (directed/orchestrated/round-robin), lead, and limits; one
      shared message log per room (sender/recipient/tool calls/token info);
      Rust agent runtime (visibility-filtered context, streaming, native tool
      calling with automatic JSON/text fallback per agent, `tool_protocol`
      column); built-in tools send_message/delegate/finish with per-member
      `can_talk_to` allowlists; orchestrated-first scheduler plus directed
      (@mention) and round-robin; guardrails (max turns, token budget,
      wall-clock timeout, loop detection, Pause/Resume/Step/Stop with "stopped
      because..." notices); rooms UI (avatars, routing lines, per-agent live
      drafts, collapsible tool blocks, token/latency meta, live SVG graph);
      everything persisted, rooms reopen and continue
- [x] **P4** Android, remote mode, external tools, release: mobile UI
      (bottom nav, inspector sheet, virtualized lists, Ollama hidden on
      phones), Android Keystore keys via keyring, foreground-service +
      cleartext config (see docs/android.md); LAN remote mode with pairing
      codes, tokens, SSE room runs, loopback/Tailscale-only binds
      (docs/remote.md); desktop-external tools off by default (MCP stdio/HTTP
      client, workspace folder, Tavily search, Docker-or-subprocess runner)
      behind per-agent permissions + allow-once/always/deny approvals with
      full audit log; release workflows (MSI/NSIS, AppImage/deb, signed
      APK/AAB), signed auto-updates, LICENSE/CONTRIBUTING/docs;
      accessibility + performance + security final pass (docs/security.md)
- [ ] **P5** RAG knowledge base (core chunking/embeddings/store already in
      `roundtable-core`: `chunk`, `embed`, `kb`)
