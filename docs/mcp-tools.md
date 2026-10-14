# MCP tools (desktop only, off by default)

Agents gain external tools through MCP (Model Context Protocol) servers.
Everything here is opt-in: no servers configured, no permissions granted,
no tools assigned → agents are chat-only and nothing changes.

## Adding a server

Settings → Tools → MCP servers → Add:

- **stdio** (runs a local command): name it, set the command
  (e.g. `npx`, `python3`, `/usr/local/bin/my-mcp`) plus args
  (e.g. `-y @my/mcp-server`). Roundtable spawns it per operation and
  kills it afterwards — no daemons.
- **HTTP** (streamable): name it, set the URL
  (e.g. `https://mcp.example.com/mcp`). Stateless calls only; servers
  that require a session handshake fail with guidance — prefer their
  stdio command if they ship one.

"View tools" spawns/lists live so you can verify the server before
assigning anything.

## Assigning tools to an agent

Agent builder (or room member editor) → Tools & permissions:

1. Tick the `server-id:tool-name` checkboxes. Assignment alone does not
   run anything — it only puts the tool on the agent's menu.
2. Grant the matching permission where needed (workspace for file tools
   you write yourself; MCP tools need no permission checkbox — the
   approval dialog is the gate).
3. Every MCP call opens an approval dialog showing the exact arguments:
   **Allow once / Always for this agent / Deny**. "Always" is stored per
   agent+tool (`agent_tool_grants`) and can be re-asked never — remove
   the assignment to stop the tool appearing at all.

Every execution — allowed, denied, or failed — lands in the audit log
(room detail → Audit log; global view in Settings).

## Writing your own server

Anything speaking MCP `2024-11-05` over stdio works. Minimal contract
Roundtable exercises:

1. `initialize` → `{protocolVersion, capabilities, serverInfo}`.
2. `tools/list` → `{tools: [{name, description, inputSchema}]}`.
3. `tools/call` with `{name, arguments}` → `{content: [{type:
   "text", text}]}` (non-text parts are noted, not transferred).

Newline-delimited JSON-RPC on stdin/stdout; 15s per-operation timeout;
unknown methods may answer `Method not found`. A 20-line Python script
that echoes arguments is enough to verify end to end — see the fake
servers in `crates/roundtable-core/src/mcp.rs` tests for the exact bytes.

## Limits (read before depending on this)

- No roots, no sampling, no elicitation: servers that request them get
  errors. Keep tools self-contained.
- Per-call processes: servers with in-memory state across calls will not
  see it persist. Prefer stateless tools.
- HTTP is stateless: no `Mcp-Session-Id` tracking.
