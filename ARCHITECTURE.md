# Oxidite Architecture

Oxidite is a browser-based terminal platform backed by a Rust session server (`oxidited`).
The desktop GPU terminal emulator (Phases 0 to 8) is frozen in `crates/oxidite-desktop`.
Its VT parser, terminal grid, and PTY abstractions form the core engine for server-side persistent sessions.

## Workspace Layout

- `crates/oxidite-vt`: Pure VT parser, terminal state, grid, and screen snapshot serializer. Zero GUI or network dependencies.
- `crates/oxidite-pty`: Cross-platform blocking PTY abstraction powered by `portable-pty`.
- `crates/oxidite-theme`: Color themes and token definitions serializable to JSON/MessagePack.
- `crates/oxidite-proto`: Wire protocol types, MessagePack binary framing, error codes.
- `crates/oxidite-session`: Async `Backend` trait, `LocalPtyBackend`, `SessionActor`, and replay ring buffer.
- `crates/oxidite-ssh`: SSH connection management, host-key verification, keepalives, and tmux persistence.
- `crates/oxidite-directory`: Distributed `SessionDirectory` trait with TTL leases and fencing tokens.
- `crates/oxidite-server`: Axum web server and WebSocket gateway binary (`oxidited`).
- `crates/oxidite-desktop`: Frozen native desktop terminal emulator.
- `web/`: Browser frontend built with TypeScript, Vite, `@xterm/xterm`, and `@msgpack/msgpack`.

## Runtime Pipeline

```text
Backend (LocalPty | SSH channel)
  -> backend reader (bounded channel, never unbounded)
  -> Session actor (single owner of Terminal state)
       ├─ VT parser -> Terminal (screen state, scrollback, modes)
       ├─ Replay buffer (ring buffer, indexed by output byte offset)
       └─ fan-out to attached clients
  -> WebSocket (binary frames, MessagePack, resumable)
  -> Browser (xterm.js)

Browser input / resize
  -> WebSocket
  -> Session actor
  -> Backend writer
  -> Shell
```

Connection layers (each fails and recovers independently):

```text
Browser  ⇄  Gateway node  ⇄  Target host
   (A)            (B)
(A) WebSocket: resumable with sequence numbers and snapshots
(B) SSH connection: keepalives, failure detection, reconnect, optional tmux
```

The session actor is the only code that mutates `Terminal`. WebSocket handlers or backends never touch terminal state directly.
