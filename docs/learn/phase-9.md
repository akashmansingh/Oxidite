# Phase 9: Workspace Split

## What Was Built

Phase 9 split the Oxidite monolithic repository into a cleanly segregated Cargo workspace of 9 specialized crates:

1. **`oxidite-vt`**: Completely decoupled VT parser, handler contract, terminal grid, and screen snapshot serializer. Contains zero GUI, windowing, or async dependencies.
2. **`oxidite-pty`**: Shared cross-platform blocking PTY abstraction powered by `portable-pty`.
3. **`oxidite-theme`**: Presets and color token management serializable to JSON/MessagePack.
4. **`oxidite-proto`**: Complete wire protocol types (`ClientMsg`, `ServerMsg`, `BackendSpec`, `SessionState`), error codes, and MessagePack framing utilities with round-trip unit tests.
5. **`oxidite-session`**: Async `Backend` trait, `LocalPtyBackend`, `SessionActor` owning terminal and replay ring buffer, and `SessionManager`.
6. **`oxidite-ssh`**: `SshBackend`, configuration, and known hosts store contracts.
7. **`oxidite-directory`**: Distributed `SessionDirectory` trait with leases, fencing tokens, and `InMemoryDirectory`.
8. **`oxidite-server`**: Axum-powered HTTP and WebSocket gateway (`oxidited`), serving web assets and bridging WebSocket clients to session actors.
9. **`oxidite-desktop`**: Frozen native desktop terminal emulator using re-exported crates.

## Key Learnings & Decisions

- **Zero Cycles:** Dependency ordering strictly flows inward (`vt, pty, theme, proto` <- `session` <- `ssh, directory` <- `server`).
- **Snapshot Escape Sequences:** `serialize_snapshot` renders terminal screen contents directly into ANSI escape sequences (`CSI 2J`, cursor moves, SGR colors), allowing any standard VT client (like xterm.js) to re-hydrate without custom client-side snapshot parsers.
- **MessagePack Bin Encoding:** Using `serde_bytes` ensures binary chunks arrive as MessagePack `bin` data types rather than array-of-integers, reducing serialization overhead by ~75%.
