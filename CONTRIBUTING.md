# Contributing to Roundtable

## Ground rules

- **Never commit secrets.** API keys live in the OS credential store
  (Keychain / Secret Service / Android Keystore), never in SQLite, logs,
  exports, or chat transcripts. The updater private key and the Android
  keystore live in CI secrets / your machine only.
- **Core stays platform-free.** `crates/roundtable-core` must compile for
  desktop and Android alike: no Tauri imports, no `std::process`-by-default
  assumptions beyond what the sandbox documents, no blocking calls in async
  code without noting it.
- **Frozen contracts.** `src/lib/api.ts` is the wire contract between the
  Rust backend and the UI. Changing a command name, param, or JSON shape
  means updating the command, the wrapper, and every caller in one commit.
- **Verify before you claim.** `npm run build` (tsc + vite),
  `cargo test -p roundtable-core`, `cargo check --workspace` with zero
  warnings. Test files stay hermetic: mock HTTP servers over loopback,
  temp dirs under `std::env::temp_dir()`, no external network.

## Workflow

1. Fork, branch from `main` (`feat/…`, `fix/…`, `docs/…`).
2. Small, reviewable commits; one concern per commit.
3. New backend behavior ships with core tests (see `rooms.rs` scripted
   mock-server tests and `db.rs` round-trip tests for the pattern).
4. New UI ships keyboard-reachable, both themes, `prefers-reduced-motion`
   respected, no new npm dependencies without discussion.
5. Open a PR describing what changed, how you verified it, and what you
   could *not* verify (device-only paths, Docker paths, etc.).

## Docs worth reading first

- `docs/adding-provider.md` — new model endpoint
- `docs/agent-templates.md` — new built-in role
- `docs/mcp-tools.md` — external tool via MCP
- `docs/remote.md` — remote-mode threat model and pairing flow
- `docs/android.md` — mobile scaffold, keystore, foreground service
- `docs/release.md` — signing keys, CI secrets, publishing a release
- `docs/security.md` — key handling, sandbox limits, audit expectations
