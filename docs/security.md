# Security review (Phase 4 final pass)

## Key handling — verified in code

- **One door in, no copies out.** All secrets (provider keys, Tavily key,
  remote-client token) go through `src-tauri/src/secrets.rs`, which wraps
  the OS store only: keyring Secret Service / Keychain on desktop,
  `android-native-keyring-store` (Keystore) on Android. Grep to confirm:
  `set_password` appears only in `secrets.rs`.
- **Memory-only reads.** Commands resolve keys into locals passed straight
  to `chat_stream` / reqwest builders. Keys never enter SQLite (all
  `INSERT`s are key-free — check `db.rs`), logs (`log::` calls carry ids
  and counts, never secrets), exports (`ExportBundle` has no key field),
  or the webview (no command returns a key; `*_has` commands return bool).
- **Remote tokens, not passwords.** The desktop stores only SHA-256 hashes
  (`hash_token`, std-only implementation in `db.rs`, FIPS vector-tested);
  the plaintext exists once at pairing and travels once over the pairing
  call. Revocation deletes the hash row — immediate 401 afterwards.
- **Pairing is narrow.** 6-digit codes, 10-minute TTL, single use, 5
  wrong attempts burns the code (`src-tauri/src/remote.rs`). No
  unauthenticated route except `/` (version probe) and `/pair`.

## Remote-mode server — reviewed

- Binds refused except loopback and Tailscale (`bind_allowed`); no
  plaintext-LAN flag exists on purpose. See `docs/remote.md` threat model.
- Minimal hand-rolled HTTP: fixed 1MB body cap, 30s read timeout, `Connection:
  close` always, per-connection threads, no keep-alive state machine to
  desync. SSE streams are write-only from the server side.
- Room runs started remotely register in the same `room_controls` map, so
  desktop pause/stop/approve apply; remote stop sets the same flag.
- Known gap (documented, not fixed): no rate limiting past pairing
  attempts. Do not expose the port beyond loopback/Tailscale.

## Tool sandbox — limits, stated plainly

- `run_code` prefers `docker run --rm --network none --memory 256m
  --cpus 1` **only when the image is already local** (never pulls from
  inside a tool call); otherwise a stripped-env subprocess
  (`env_clear`, `PATH=/usr/bin:/bin`, temp-dir cwd, stdin null, 30s
  kill). The subprocess path is best-effort containment, NOT a security
  boundary — it cannot block network egress without namespaces.
- Workspace tools jail paths lexically (`workspace_path`: no absolutes,
  `..` cannot escape the picked folder) and the folder itself is
  user-picked per install.
- Writes, code runs, and every MCP call require a human decision unless
  the agent holds an explicit "always" grant; denials and failures are
  audited with the same row shape as approvals (`tool_audit`).
- MCP servers run with the user's own privileges — a malicious server
  sees whatever its command can see. Only add servers you trust, keep
  them disabled when idle, and scope assignments per agent.

## Audit expectations

- Every tool execution writes one `tool_audit` row: time, room, agent,
  tool, args (2k cap), decision (`allow`/`once`/`always`/`grant`/`deny`/
  `deny-permission`), result summary (500 chars). Audit writes are
  best-effort (`let _`) so a full disk fails the tool, never the run
  silently — the error surfaces in the tool result.
- Audit rows are local-only: exports exclude them by design (check
  `ExportBundle`), so device pairing never leaks tool history.
