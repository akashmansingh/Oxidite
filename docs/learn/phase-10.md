# Phase 10: Server Skeleton, Protocol, and Browser Terminal

## What Was Built

Phase 10 implemented the core server daemon (`oxidited`), wire protocol communication over WebSockets, and client-side xterm.js integration:

1. **Wire Encoding & Handshake:**
   - MessagePack named binary framing with `rmp_serde` and `serde_bytes`.
   - First-message `Hello` handshake with protocol version validation (`PROTOCOL_VERSION = 1`) and bearer token verification.
   - Server responds with `Welcome { server_version }` or terminates on mismatch.

2. **Session Actor & PTY Bridge:**
   - Each session is owned by an isolated tokio actor task.
   - The actor is the single owner of the `Terminal` grid and `ReplayBuffer`.
   - Shell execution is bridged to the async runtime via background reader threads and bounded channels (`mpsc::channel(128)`).
   - User input and window resize messages are dispatched from WebSocket frames to the session actor and shell.

3. **Browser Terminal Client:**
   - Client created using Vite, TypeScript, `@xterm/xterm`, and `@xterm/addon-fit`.
   - Wire protocol client (`connection.ts`) handles WebSocket connections, MessagePack packing/unpacking, ping/pong heartbeats, and input streaming.
   - Built frontend assets served by `oxidited` at root and `/assets/*` with SPA fallback.

## Key Learnings & Decisions

- **Single Sequencer:** The session actor assigns cumulative byte offsets to all output bytes as they arrive from the backend, guaranteeing a deterministic, monotonic stream across disconnects.
- **Loopback Default:** Security by default dictates binding to `127.0.0.1:3000` with an auto-generated high-entropy bearer token.
