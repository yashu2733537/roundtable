# Remote mode: desktop server + Android client

## Threat model

The server exposes your agents, rooms, and local models to the network it
binds. Design rules, enforced in code:

1. **Token on everything except pairing.** Every route but `POST /pair`
   requires `Authorization: Bearer <token>`; tokens are random 256-bit,
   stored hashed (SHA-256) server-side, and shown once at pairing.
2. **No plaintext on the open LAN.** `remote_start` accepts loopback
   (`127.0.0.1`, `::1`, `localhost`) and Tailscale (`.ts.net`,
   `100.64.0.0/10` — encrypted tunnel) binds only. Anything else is
   refused with guidance. There is intentionally no "I accept the risk"
   flag for raw LAN: bind 127.0.0.1 behind your own TLS proxy instead.
3. **Pairing is the only weak point, and it is narrow.** 6-digit codes,
   10-minute expiry, single use, 5 wrong attempts burns the code. Pair
   with the screen visible, on a network you trust.
4. **Clients hold tokens in the OS store.** Android: Keystore via the
   secure-storage plugin; desktop testing: same credential store as
   provider keys. Tokens never appear in logs, exports, or the UI after
   pairing (only the device label + date do).
5. **Unpairing is instant.** Settings → Remote → revoke the device; the
   hash row is deleted and the next call 401s.

## Pairing (phone ↔ desktop)

1. Desktop: Settings → Remote → Start (default 127.0.0.1:8420 — switch
   the bind to your Tailscale IP for phone access).
2. Desktop: "New pairing code" → 6 digits + countdown.
3. Phone: Settings → Remote → server URL (`http://100.x.y.z:8420`),
   device name, code → Connect. Ten minutes, five tries.
4. Phone rooms/agents screens now drive the desktop runtime over SSE.
   Stop a runaway from either side (Stop also works over
   `POST /rooms/:id/stop`).

## Endpoints (all JSON unless noted; auth as above)

| Method + path | Body → result |
| --- | --- |
| `GET /` | probe, no auth: `{name, version}` |
| `POST /pair` | `{code, label}` → `{token, id}` (once) |
| `GET /agents` | agent list (no keys — keys never leave the desktop) |
| `GET /rooms` | room list |
| `GET /rooms/:id/messages` | room log |
| `GET /rooms/:id/state` | run state |
| `POST /rooms/:id/start` | `{content?}` → SSE `RoomEvent` stream |
| `POST /rooms/:id/stop` | `{stopped}` |

SSE frames are `data: <RoomEvent JSON>`; the stream closes when the run
ends. RoomEvent shapes match `src/lib/api.ts` exactly.

## What remote does NOT do (v1)

- No remote approvals: a desktop run that hits an approval waits for the
  desktop dialog. Start risky runs at the desk, or pre-grant "always".
- No file transfer: workspace tools execute on the desktop's folder.
- No TLS termination: bring Tailscale or your own proxy.
