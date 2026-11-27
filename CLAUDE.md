# CLAUDE.md — Oxidite Project Guide & Developer Roadmap

## Project Overview

**Oxidite** is a browser- and mobile-accessible terminal platform backed by a Rust session server (`oxidited`).

Sessions live on the server, survive browser disconnects and network drops, connect to local shells (including PowerShell with live CWD tracking) or remote machines over SSH (`russh` + optional `tmux`), and expose session-scoped Git inspection and multi-device pairing over an authenticated, resumable MessagePack WebSocket protocol.

*(The legacy native `oxidite-desktop` GPU app has been removed from the workspace; its VT parser, terminal grid, PTY abstraction, and themes live in shared server crates.)*

---

## Current Goals & Active Focus

1. **Remote & Mobile Web Experience**
   - Seamless access to local workspaces and remote SSH hosts from desktop browsers and mobile devices (via Tailscale Serve, Cloudflare Tunnel, or LAN TLS).
   - Multi-device enrollment via single-use `#pair=` grants, Argon2id-hashed `od1.` device tokens, and client-side WebCrypto + IndexedDB `DeviceVault` protected by a user PIN.
2. **Dynamic Workspace & Git Cockpit**
   - Live child-process / OSC `9;9` / OSC `7` CWD tracking per session.
   - Session-scoped `/api/git/*` endpoints (`changes`, `diff`, `log`, `branches`, `stage`, `commit`, `checkout`, `fetch`) confined by `OXIDITE_WORKSPACE_ROOTS` and path-traversal guards.
3. **Security & Hardening**
   - Default loopback bind on **`127.0.0.1:3030`**.
   - Mandatory TLS (`OXIDITE_TLS_CERT` / `OXIDITE_TLS_KEY`) on non-loopback binds.
   - `Origin` validation on WebSocket upgrades, `OXIDITE_ALLOW_LOCAL_PTY` gating, rate limiting, and zero secret/keystroke logging.

---

## Core Pipeline & Architecture

```text
Backend (LocalPty | russh SSH channel)
  │
  ▼ (bounded channel, backpressure at high/low watermarks)
SessionActor (single owner of Terminal state, ReplayBuffer, CWD)
  │
  ▼ (64 KiB coalesced MessagePack frames over WebSocket)
Axum Gateway (oxidited, default port 3030)
  │
  ▼
Browser / Mobile Web Client (web/: TypeScript + Vite + xterm.js + @msgpack/msgpack)
```

---

## Architectural Invariants

1. **Session lifetime is separate from connection lifetime.** Sessions survive WebSocket disconnects and restore via byte-offset replay or VT repaint snapshots.
2. **Single owner per session.** One `SessionActor` task owns each session's `Terminal`, replay buffer, and backend.
3. **Bounded everything.** Every channel, replay buffer (default 8 MiB), and log sink has explicit size limits.
4. **Never block the async runtime.** Blocking PTY reads, Git subprocesses, Argon2 hashing, and SQLite/filesystem checks run on dedicated threads or `spawn_blocking`.
5. **Secrets never reach logs.** Tokens, PINs, passwords, private keys, and terminal output are never logged.
6. **No fake stubs.** All Git, shell, SSH, and device states reflect real backend state.

---

## Workspace Layout

| Path | Primary Responsibility |
|---|---|
| `crates/oxidite-vt` | VT parser (`vte`), terminal state, grid, scrollback, repaint snapshot serializer |
| `crates/oxidite-pty` | Cross-platform `portable-pty` wrapper, safe shell resolution, PowerShell fallback & live CWD probe |
| `crates/oxidite-theme` | Theme presets and color tokens |
| `crates/oxidite-proto` | Wire protocol (`ClientMsg`, `ServerMsg`, MessagePack framing, `SessionState`) |
| `crates/oxidite-session` | `Backend` trait, `LocalPtyBackend`, `SessionActor`, `SessionManager`, replay ring buffer |
| `crates/oxidite-ssh` | `russh` SSH client, TOFU `KnownHostsStore`, keepalives, reconnect, connection pool, `tmux` integration |
| `crates/oxidite-directory` | `SessionDirectory` trait, leases, fencing tokens, partition simulation |
| `crates/oxidite-server` | `oxidited` binary: Axum HTTP/WS server, TLS, device auth, security middleware, Git API, Prometheus metrics |
| `web/` | Browser & mobile UI (`index.html`, `src/main.ts`, `src/protocol.ts`, `src/connection.ts`, `src/device-vault.ts`, `src/style.css`) |

---

## Common Commands

```bash
# Build web frontend (served from web/dist by oxidited)
cd web && bun install && bun test && bun run build && cd ..

# Run gateway server on http://127.0.0.1:3030
cargo run -p oxidite-server --bin oxidited

# Full Rust CI checks
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo check --benches
```
