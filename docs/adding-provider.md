# Adding a provider

Providers are OpenAI-compatible endpoints (`GET /models`,
`POST /chat/completions` with SSE). No code changes needed for a standard
one — everything is UI + credential store.

1. Open Settings → Providers → Add.
2. Name it (e.g. "Together"), set the base URL to the OpenAI-compatible
   root (e.g. `https://api.together.xyz/v1`), paste the key.
3. The key lands in the OS credential store (`app.roundtable.desktop` /
   Android Keystore) — never in SQLite. "Saved" appears next to the row.
4. Pick models from the dropdown (fetched live from `/models`) or type a
   model id by hand.

## Non-standard endpoints

- **Ollama** is built in (id `ollama`, no key): the app probes
  `http://localhost:11434` and lists installed models. Start Ollama first.
- **Auth schemes other than Bearer** are not supported — the shell sends
  `Authorization: Bearer <key>` (see `resolve_provider` in
  `src-tauri/src/commands.rs`). Open an issue with the scheme and a link
  to the endpoint docs.
- **Native tool calling** is probed per agent: endpoints that reject the
  `tools` parameter fall back to the JSON/text protocol automatically and
  remember the choice (`tool_protocol` column).

## Code pointers

- `crates/roundtable-core/src/providers.rs` — `/models` parsing, SSE
  streaming, usage capture, tool-call accumulation.
- `src-tauri/src/secrets.rs` — key storage rules (read before touching).
- `src/components/ModelPicker.tsx` — grouped dropdown + manual id field.
