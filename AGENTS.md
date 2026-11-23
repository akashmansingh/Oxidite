# Oxidite

## Mission

Build **Oxidite**, a browser-based terminal platform backed by a Rust session server.

The browser is the client. The server owns the shells. Sessions live on the server, survive browser disconnects, connect to remote machines over SSH, keep those SSH connections healthy, and can be shared between users and spread across multiple server nodes.

Oxidite started as a native GPU terminal emulator (Phases 0 to 8, complete). That desktop app is now frozen. Its VT parser, terminal grid, scrollback, and PTY code become the server-side engine that makes persistent, resumable sessions possible.

The system must:

1. Run a real shell (local PTY or SSH) inside a server-owned session.
2. Stream session output to a browser over a resumable WebSocket protocol.
3. Keep the session alive when the browser disconnects, and restore the exact screen when it returns.
4. Open and maintain SSH connections with host-key verification, keepalives, failure detection, and reconnection.
5. Support multiple clients on one session with a single ordered stream.
6. Support multiple server nodes with session ownership, leases, and failover.
7. Be secure by default, observable, and verified by failure-injection tests.

## Non-Goals

Do not reduce the project to:

- a wrapper around `ttyd`, `wetty`, `gotty`, Apache Guacamole, Teleport, or any existing web-terminal server
- a program that shells out to the system `ssh` binary (SSH goes through `russh` so Oxidite controls authentication, host keys, keepalives, and reconnection)
- a thin WebSocket pipe with no session state (the session, replay buffer, and screen snapshot are the product)
- a rewrite or port of the desktop GPU renderer to the browser
- a multi-tenant SaaS with billing, teams, or admin dashboards
- a fake system: never fake connection state, metrics, session ownership, or failover results

Allowed to delegate: browser rendering and key encoding to xterm.js; the SSH protocol to `russh`; HTTP and WebSocket plumbing to `axum`. Session lifecycle, the wire protocol, replay and snapshot logic, connection maintenance, ownership and leasing, and security policy are written in this repository.

A human will study this code later. Every non-trivial piece must be readable and explainable.

## Current Status

|Phases|Scope|State|
|---|---|---|
|0 to 8|Native desktop terminal (PTY, parser, grid, renderer, input, config, themes, tabs, splits, workspaces, git/diff UI)|Complete, frozen|
|9 to 17|Browser client, session server, SSH, connection maintenance, security, sharing, multi-node, durability|This document|

Rules for the desktop app:

- Do not add features to `oxidite-desktop`.
- Fix only breakage caused by workspace refactors.
- It must keep building and passing tests in CI.

## Core Pipeline

Server data flow:

```
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

```
Browser  ⇄  Gateway node  ⇄  Target host
   (A)            (B)
(A) WebSocket: resumable with sequence numbers and snapshots
(B) SSH connection: keepalives, failure detection, reconnect, optional tmux
```

The session actor is the only code that mutates `Terminal`. Do not let WebSocket handlers or backends touch terminal state directly.

## Architectural Invariants

1. **Session lifetime is separate from connection lifetime.** A session outlives its WebSocket and, where possible, its SSH connection.
2. **Single owner per session.** One tokio task owns each session's `Terminal`, replay buffer, and backend. Everything else talks to it through messages.
3. **Single sequencer.** The owning actor assigns every output byte its offset. No other component orders session events.
4. **Bounded everything.** Every channel, buffer, queue, and log has a size limit and a defined overflow policy. No unbounded channels.
5. **Never block the async runtime.** Blocking work (PTY reads, file IO, password hashing) runs on dedicated threads or `spawn_blocking`.
6. **Every network operation has a timeout.**
7. **Slow clients never stall a session.** Apply backpressure or resync from a snapshot; never buffer without limit.
8. **Honest guarantees.** Do not claim a guarantee that a test does not prove. Document failure semantics in `docs/guarantees.md`.
9. **Secrets never reach logs.** No keystrokes, passwords, private keys, tokens, or session output in logs or error messages.
10. **No fake stubs.**

## Shared Source of Truth

Read these before implementation. Create any missing file in Phase 9.

- `docs/plan.md` — architecture, phase checklist, integration status
- `docs/contracts/shared-contracts.md` — cross-crate types and traits
- `docs/contracts/wire-protocol.md` — WebSocket protocol specification
- `docs/guarantees.md` — what is and is not guaranteed under each failure
- `docs/threat-model.md` — assets, attackers, mitigations
- `docs/plans/session.md`, `ssh.md`, `server.md`, `web.md`, `directory.md` — component plans
- `docs/integration.md` — merge and integration checkpoints
- `docs/runbooks/demo.md` — golden demo definition
- `docs/runbooks/chaos.md` — failure-injection procedures
- `docs/learn/phase-N.md` — plain-language notes written after each phase
- `ARCHITECTURE.md`, `README.md`, `CLAUDE.md` — update as the architecture changes

External references:

- `russh` docs and examples (client, interactive PTY client)
- Warpgate and Devolutions Gateway source — reference for how SSH gateways are built
- `axum` WebSocket docs; `tokio` docs (channels, cancellation, timeouts)
- xterm.js docs, including the flow-control guide
- RFC 4254 (SSH connection protocol: channels, `pty-req`, `window-change`)
- OpenSSH `ControlMaster`, `ServerAliveInterval` behavior
- tmux man page (`new-session -A`, `attach-session`)
- mosh paper (state synchronization) and Eternal Terminal docs
- "Designing Data-Intensive Applications" chapters on leases, fencing, and replication

## Tech Stack

|Concern|Choice|
|---|---|
|Async runtime|`tokio`|
|HTTP + WebSocket|`axum`|
|Wire encoding|MessagePack via `rmp-serde` (chosen because browsers have mature MessagePack libraries)|
|IDs|`uuid`|
|SSH client|`russh` (enable exactly one crypto backend feature)|
|Local PTY|`portable-pty` (blocking; bridge to tokio)|
|VT parsing|`vte` (existing)|
|Database|`sqlx` (SQLite first, Postgres for multi-node)|
|TLS|`rustls`|
|Password hashing|`argon2`|
|Secrets in memory|`secrecy`, `zeroize`|
|Observability|`tracing`, `tracing-subscriber`, Prometheus metrics exporter|
|Rate limiting|`tower` middleware|
|Errors|`thiserror` (libraries), `anyhow` (binaries)|
|Frontend|TypeScript, Vite, `@xterm/xterm`, fit addon, WebGL addon, `@msgpack/msgpack`|
|Desktop (frozen)|`winit`, `wgpu`, `glyphon`|

Rules:

- Check current versions and compatibility before pinning. `russh` is pre-1.0 and requires a crypto backend feature.
- Check current npm package names for xterm.js and its addons before installing.
- Ask before adding any dependency not listed here.
- Pin versions. Commit `Cargo.lock` and the frontend lockfile.

## Package Boundaries

Any contributor may work anywhere in the repository. Keep changes coherent with these component boundaries, but do not treat them as exclusive ownership.

- **VT** (`crates/oxidite-vt`): parser, handler trait, terminal, grid, scrollback, snapshot serialization. No GUI, network, or async dependencies.
- **PTY** (`crates/oxidite-pty`): blocking `portable-pty` abstraction shared by desktop and server.
- **Theme** (`crates/oxidite-theme`): theme structs and built-in presets, serializable to JSON for the web client.
- **Protocol** (`crates/oxidite-proto`): message types, framing, versioning. No IO.
- **Session** (`crates/oxidite-session`): `Backend` trait, `LocalPty` backend, session actor, session manager, replay buffer, state machine, flow control.
- **SSH** (`crates/oxidite-ssh`): `russh` wrapper, auth, host-key store, keepalive, reconnect, connection pool, tmux integration, `SshBackend`.
- **Directory** (`crates/oxidite-directory`): `SessionDirectory` trait, in-memory and Postgres implementations, leases, fencing tokens.
- **Server** (`crates/oxidite-server`, binary `oxidited`): axum app, WebSocket handler, auth, TLS, routing/proxying, audit log, metrics.
- **Web** (`web/`): xterm.js client, protocol client, reconnect logic, connect forms, status UI.
- **Desktop** (`crates/oxidite-desktop`): the frozen native app.

Dependency direction (no cycles): `vt, pty, theme, proto` <- `session` <- `ssh, directory` <- `server`. The desktop crate depends only on `vt`, `pty`, and `theme`.

## Repository Layout

```
oxidite/
├── AGENTS.md
├── Cargo.toml                    # workspace root
├── Cargo.lock
├── README.md
├── ARCHITECTURE.md
├── docs/
│   ├── plan.md
│   ├── guarantees.md
│   ├── threat-model.md
│   ├── integration.md
│   ├── contracts/{shared-contracts,wire-protocol}.md
│   ├── plans/{session,ssh,server,web,directory}.md
│   ├── runbooks/{demo,chaos}.md
│   └── learn/phase-N.md
├── crates/
│   ├── oxidite-vt/
│   ├── oxidite-pty/
│   ├── oxidite-theme/
│   ├── oxidite-proto/
│   ├── oxidite-session/
│   ├── oxidite-ssh/
│   ├── oxidite-directory/
│   ├── oxidite-server/
│   └── oxidite-desktop/
├── web/
│   ├── package.json
│   ├── vite.config.ts
│   └── src/{main.ts,protocol.ts,connection.ts,terminal.ts,ui/}
├── deploy/
│   ├── Dockerfile
│   └── docker-compose.yml        # gateway nodes, postgres, test sshd
└── tests/
    ├── chaos/                    # failure-injection scripts and tests
    └── fixtures/
```

## Shared Contracts

These are starting contracts. Put them in `docs/contracts/shared-contracts.md` and as code stubs first. Any change goes through its own `contract/*` branch and merges before dependent work continues.

```rust
// oxidite-proto
pub type SessionId = uuid::Uuid;
pub type Offset = u64;                 // cumulative output byte offset within a session

pub struct TermSize { pub rows: u16, pub cols: u16 }

pub enum ClientMsg {
    Hello { protocol_version: u16, token: String },
    Create { backend: BackendSpec, size: TermSize },
    Attach { session: SessionId, last_offset: Option<Offset>, mode: AttachMode },
    Input(Vec<u8>),
    Resize(TermSize),
    Ack { offset: Offset },
    HostKeyDecision { accept: bool },
    AuthResponse { answers: Vec<String> },
    Ping { nonce: u64 },
    Detach,
    Kill,
}

pub enum ServerMsg {
    Welcome { server_version: String },
    Created { session: SessionId },
    Attached { session: SessionId, mode: AttachMode, start: ReplayStart },
    Snapshot { offset: Offset, bytes: Vec<u8> },   // repaint escape sequences
    Output { offset: Offset, bytes: Vec<u8> },     // offset = start of this chunk
    State { session: SessionId, state: SessionState },
    HostKeyPrompt { host: String, fingerprint: String, known: HostKeyStatus },
    AuthPrompt { prompts: Vec<String> },
    Title(String),
    Exit { code: Option<i32> },
    Pong { nonce: u64 },
    Error { code: ErrorCode, message: String },
}

pub enum AttachMode { Writer, Viewer }
pub enum ReplayStart { Replay { from: Offset }, Snapshot { at: Offset } }

pub enum BackendSpec {
    LocalPty { shell: Option<String>, cwd: Option<String> },
    Ssh {
        host: String, port: u16, user: String,
        auth: AuthSpec, multiplexer: Multiplexer,
    },
}
pub enum Multiplexer { None, Tmux }

pub enum SessionState {
    Starting,
    Running,
    Degraded,                       // keepalive misses, connection suspect
    Reconnecting { attempt: u32 },
    Detached,                       // running with no clients attached
    Exited { code: Option<i32> },
    Failed { reason: String },
}

// oxidite-session
#[async_trait]
pub trait Backend: Send {
    async fn write(&mut self, bytes: &[u8]) -> Result<()>;
    async fn resize(&mut self, size: TermSize) -> Result<()>;
    fn events(&mut self) -> mpsc::Receiver<BackendEvent>;   // bounded
    async fn kill(&mut self) -> Result<()>;
}
pub enum BackendEvent {
    Output(Vec<u8>),
    State(SessionState),
    Exited(Option<i32>),
}

// oxidite-directory
#[async_trait]
pub trait SessionDirectory: Send + Sync {
    async fn claim(&self, session: SessionId, node: NodeId, ttl: Duration) -> Result<Lease>;
    async fn renew(&self, lease: &Lease, ttl: Duration) -> Result<Lease>;
    async fn release(&self, lease: Lease) -> Result<()>;
    async fn lookup(&self, session: SessionId) -> Result<Option<Owner>>;
    async fn list_orphans(&self) -> Result<Vec<SessionId>>;
}
pub struct Lease { pub session: SessionId, pub node: NodeId, pub fencing_token: u64, pub expires_at: Instant }
```

## Wire Protocol Rules

Full spec lives in `docs/contracts/wire-protocol.md`. Non-negotiable rules:

- Binary WebSocket frames, MessagePack encoded.
- The first client message must be `Hello` with a `protocol_version`. Reject mismatches with a clear error.
- `Offset` is the cumulative byte offset of the session output stream. Acks and replay are expressed in offsets.
- Server output frames are coalesced to at most 64 KiB.
- **Replay buffer:** a per-session ring buffer, default 8 MiB. On `Attach` with a `last_offset` still inside the buffer, replay the missing bytes. Otherwise send a `Snapshot` (repaint bytes generated from `Terminal`) and continue from there.
- **Snapshots are repaint escape sequences**, so the browser needs no special code path to apply them.
- **Flow control:** the client sends `Ack { offset }` as it consumes data. The server stops reading from the backend when unacked bytes exceed a high watermark (default 1 MiB) and resumes below a low watermark (default 256 KiB).
- **Attach is idempotent.** Repeating the same `Attach` produces the same result.
- **Input is at-most-once.** After a reconnect, unsent input is dropped, never replayed. Replaying keystrokes can run commands the user did not intend.
- Heartbeats: `Ping`/`Pong` both directions. Close connections that miss the configured number of heartbeats.

## Failure Semantics

Document and test every row. `docs/guarantees.md` must match this table and be backed by tests.

|Failure|What happens|Recovery|
|---|---|---|
|Browser tab closes or network drops|Session keeps running, state `Detached`|Client reattaches with `last_offset`; replay or snapshot|
|Gateway-to-target TCP drops, **tmux in use**|SSH channel lost, remote shell survives|State `Reconnecting`; reconnect and reattach to tmux; terminal state reset and redrawn|
|Gateway-to-target TCP drops, **no tmux**|Remote shell receives SIGHUP|State `Failed`; user starts a new session|
|Gateway node crashes, SSH + tmux|Connection and local state lost; lease expires|Another node adopts the orphan: reconnect, tmux reattach, new snapshot|
|Gateway node crashes, local PTY|Session and process lost|Session marked `Failed`; documented limitation|
|Network partition between node and directory|Node cannot renew lease|Node stops accepting writes for affected sessions; fencing token prevents a stale owner from acting|
|Slow client|Backpressure at high watermark; viewers resync from snapshot|No unbounded memory growth|

An SSH session cannot be resumed once its TCP connection dies. Surviving that requires a remote multiplexer (tmux first, a remote agent later). Never imply otherwise in code, docs, or UI.

## SSH Requirements

- Use `russh` for all SSH. Never spawn the system `ssh`.
- Auth: password, public key, keyboard-interactive (prompts relayed to the browser). Agent forwarding is off by default.
- Host keys: strict by default. Unknown host -> send `HostKeyPrompt`, store the fingerprint on acceptance. Changed key -> refuse the connection and report it clearly.
- Keepalive: send periodic keepalive requests (default every 15 s). After a configurable number of misses (default 3), mark the session `Degraded`, then begin reconnecting.
- Reconnect: exponential backoff with full jitter (default 1 s to 30 s), configurable max attempts, surfaced to the client as `Reconnecting { attempt }`.
- Pooling: one SSH connection per (user, host), many channels, like OpenSSH `ControlMaster`.
- Resize: forward browser resizes as SSH `window-change`.
- tmux: default multiplexer for SSH sessions is `tmux new-session -A -s oxidite-<session-id>`. If tmux is missing, continue without it and surface a clear "no persistence" warning.
- Credentials: encrypted at rest; held in memory with `secrecy`/`zeroize`; never logged.

## Security Requirements

A web terminal is a remote-code-execution endpoint. Treat every item as mandatory, and record the reasoning in `docs/threat-model.md`.

- TLS required on any non-loopback bind.
- Check the `Origin` header on the WebSocket upgrade (prevents cross-site WebSocket hijacking).
- Authentication on every connection. Start with a generated bearer token bound to loopback; move to user accounts with Argon2, then optional OIDC or WebAuthn.
- Per-user ownership check on every attach, kill, and list.
- `LocalPty` backend disabled on non-loopback binds unless explicitly enabled. Never run local shells for multiple users without sandboxing.
- Rate limit login and WebSocket connects.
- Resource limits: max sessions per user, max replay buffer, max message size.
- Audit log: who connected to what, when, from where. Never record keystrokes or output.
- Security headers and CSP on the web app.
- Share links (Phase 15) are scoped, read-only by default, expiring, and revocable.

## Build Phases

Work through the phases in order. Each phase ends with a tag, a `docs/learn/phase-N.md`, and passing checks. A phase with fewer than 8 commits is under-committed. Phases 0 to 8 (the desktop terminal) are complete.

### Phase 9 — Workspace split

Exit: `cargo test --workspace` passes, the desktop app runs unchanged, and `oxidite-vt` compiles with no GUI dependencies.

Commits:

1. `chore(workspace): convert repository to a cargo workspace`
2. `refactor(vt): extract handler trait into oxidite-vt`
3. `refactor(vt): move parser, terminal, and grid into oxidite-vt`
4. `refactor(vt): remove windowing and GPU dependencies from oxidite-vt`
5. `refactor(pty): extract pty module into oxidite-pty`
6. `refactor(theme): extract theme module into oxidite-theme`
7. `refactor(desktop): move app, renderer, and UI modules into oxidite-desktop`
8. `fix(desktop): repair imports so the desktop app builds`
9. `test(vt): confirm existing parser and grid tests pass in the new crate`
10. `chore(ci): run fmt, clippy, and test across the workspace`
11. `docs(contracts): add shared contracts and wire protocol drafts`
12. `docs: update ARCHITECTURE.md and README for the workspace`
13. `docs(learn): add phase 9 notes`

### Phase 10 — Server skeleton, protocol, and a browser terminal

Exit: `oxidited` serves a web page; a local bash session runs in the browser with working colors, vim, and resize; loopback bind only; token required.

Commits:

1. `chore(deps): add tokio, axum, rmp-serde, uuid, and tracing`
2. `feat(proto): add ClientMsg and ServerMsg types`
3. `feat(proto): add MessagePack framing with round-trip tests`
4. `feat(session): add the Backend trait`
5. `feat(session): add LocalPty backend with a blocking-thread bridge`
6. `feat(session): add the session actor owning Terminal and backend`
7. `feat(session): add SessionManager for create, get, list, and kill`
8. `feat(server): add axum app with a health endpoint`
9. `feat(server): add WebSocket upgrade and Hello handshake`
10. `feat(server): add startup bearer token and loopback-only bind`
11. `feat(server): route Create, Input, Resize, and Output through the session actor`
12. `feat(web): scaffold Vite and TypeScript project`
13. `feat(web): add xterm.js terminal with the fit addon`
14. `feat(web): implement the protocol client`
15. `feat(web): send input and resize, render output`
16. `feat(server): serve the built web assets`
17. `test(server): integration test driving a session over WebSocket`
18. `docs(learn): add phase 10 notes`

### Phase 11 — Detach and reattach

Exit: closing the browser tab during `seq 1 100000` and reopening it shows an identical screen with no gaps or duplicates; a slow client cannot grow server memory without bound.

Commits:

1. `feat(session): track cumulative output offsets`
2. `feat(session): add ring-buffer replay store with configurable capacity`
3. `feat(proto): add Attach, Ack, and Detach handling`
4. `feat(session): replay missing bytes on attach`
5. `feat(vt): serialize screen state into repaint bytes`
6. `feat(session): fall back to a snapshot when last_offset is outside the buffer`
7. `feat(session): keep sessions running when detached, with idle timeout`
8. `feat(session): add ack-based flow control with high and low watermarks`
9. `feat(server): add ping and pong with dead-client detection`
10. `feat(server): add REST endpoint to list sessions`
11. `feat(web): reconnect with exponential backoff and resume from last offset`
12. `feat(web): add connection status indicator`
13. `perf(session): bound memory under slow clients`
14. `test(session): property test that random disconnects converge to the same screen`
15. `test(session): verify snapshot equals replayed state`
16. `docs(guarantees): document attach and replay guarantees`
17. `docs(learn): add phase 11 notes`

### Phase 12 — SSH backend

Exit: the browser connects to a containerized sshd, vim works, resize works, an unknown host key prompts, and a changed host key is refused.

Commits:

1. `chore(deps): add russh with a crypto backend feature`
2. `feat(ssh): add SshConfig and auth method types`
3. `feat(ssh): connect with password authentication`
4. `feat(ssh): add public-key authentication`
5. `feat(ssh): relay keyboard-interactive prompts to the client`
6. `feat(ssh): open a channel and request a PTY and shell`
7. `feat(ssh): forward resize as window-change`
8. `feat(ssh): add known-hosts store with trust-on-first-use prompt`
9. `feat(ssh): refuse changed host keys`
10. `feat(session): add SshBackend implementing Backend`
11. `feat(proto): add SSH backend spec, host-key prompt, and auth prompt messages`
12. `feat(server): add encrypted-at-rest credential store`
13. `feat(web): add SSH connect form and host-key confirmation dialog`
14. `test(ssh): integration tests against a containerized sshd`
15. `docs(learn): add phase 12 notes`

### Phase 13 — Connection maintenance

Exit: killing the TCP connection mid-session shows `Reconnecting`, then resumes with the shell state intact when tmux is in use; without tmux the session ends in a clear `Failed` state.

Commits:

1. `feat(session): add SessionState machine with explicit transitions and tests`
2. `feat(ssh): send keepalive requests with configurable interval and miss count`
3. `feat(ssh): detect a dead connection and enter Degraded`
4. `feat(ssh): reconnect with exponential backoff and full jitter`
5. `feat(ssh): add a connection pool keyed by user and host`
6. `feat(ssh): start sessions inside tmux with new-session -A`
7. `feat(ssh): detect a missing tmux and surface a no-persistence warning`
8. `feat(session): reset terminal state when the backend reattaches`
9. `feat(proto): publish State messages to attached clients`
10. `feat(web): show Reconnecting, Degraded, and Failed states in the UI`
11. `feat(session): add idle timeout and maximum lifetime policies`
12. `test(chaos): drop TCP mid-session and assert reconnect plus tmux reattach`
13. `test(chaos): without tmux, assert a clear Failed state and no silent hang`
14. `docs(runbook): add chaos procedures and verify the failure table`
15. `docs(learn): add phase 13 notes`

### Phase 14 — Authentication and security hardening

Exit: the threat model is written, every mandatory security item has a test or a documented reason, and non-loopback binds require TLS and real auth.

Commits:

1. `feat(server): add rustls TLS configuration`
2. `feat(server): enforce an Origin allowlist on WebSocket upgrade`
3. `feat(server): add user model with Argon2 password hashing`
4. `feat(server): issue short-lived rotating session tokens`
5. `feat(server): enforce per-user ownership on attach, kill, and list`
6. `feat(server): rate limit login and WebSocket connects`
7. `feat(server): add audit log without keystrokes or output`
8. `feat(server): disable LocalPty on non-loopback binds by default`
9. `feat(server): enforce session, buffer, and message size limits`
10. `feat(server): add CSP and security headers`
11. `feat(server): add optional OIDC login`
12. `test(security): origin rejection, ownership checks, and rate limits`
13. `docs: write the threat model`
14. `docs(learn): add phase 14 notes`

### Phase 15 — Shared sessions

Exit: several browsers attach to one session; exactly one is the writer; all converge to the same screen; a slow viewer never blocks the session.

Commits:

1. `feat(session): add Writer and Viewer attach modes`
2. `feat(session): enforce a single writer with takeover requests`
3. `feat(session): fan out output to N clients with independent ack windows`
4. `feat(session): resync slow viewers from a snapshot instead of blocking`
5. `feat(proto): add presence messages`
6. `feat(server): add scoped, expiring, revocable share tokens`
7. `feat(web): add viewer mode and share-link UI`
8. `test(session): N clients converge to an identical screen`
9. `test(session): a stalled viewer does not delay the writer`
10. `docs(learn): add phase 15 notes`

### Phase 16 — Multi-node ownership and failover

Exit: with two gateway nodes, killing the owner of an SSH+tmux session lets the other node adopt it and the client resume; partitioning lease renewals never produces two writers.

Commits:

1. `feat(directory): add the SessionDirectory trait and in-memory implementation`
2. `feat(directory): add Postgres implementation with TTL leases`
3. `feat(directory): add fencing tokens`
4. `feat(server): add node identity and heartbeats`
5. `feat(server): claim a lease on session create and renew it in the background`
6. `feat(server): reject writes carrying a stale fencing token`
7. `feat(server): proxy WebSocket traffic to the owning node`
8. `feat(server): detect orphaned sessions when leases expire`
9. `feat(session): adopt orphaned SSH sessions on another node`
10. `feat(server): drain gracefully on shutdown and hand off leases`
11. `test(chaos): kill the owner node and verify adoption and client resume`
12. `test(chaos): partition lease renewals and verify no split-brain`
13. `docs(guarantees): document the multi-node failure model`
14. `docs(learn): add phase 16 notes`

### Phase 17 — Durability, observability, and release

Exit: the golden demo passes from `docker compose up`; chaos tests run in CI; the README states guarantees honestly.

Commits:

1. `feat(log): add an append-only, segmented per-session output log`
2. `feat(log): add snapshot and compaction`
3. `feat(log): add replay API and asciicast export`
4. `feat(obs): add tracing spans per session`
5. `feat(obs): add Prometheus metrics for sessions, reconnects, replay lag, and RTT`
6. `feat(obs): add health and readiness endpoints`
7. `feat(deploy): add Dockerfile and docker-compose with gateways, postgres, and a test sshd`
8. `test(load): benchmark many concurrent sessions`
9. `chore(ci): run workspace tests and containerized integration and chaos tests`
10. `docs: rewrite README with architecture, security, and deployment`
11. `docs: add architecture diagrams`
12. `chore: add release workflow`
13. `docs(learn): add final notes`

## Git Rules

- Work on separate feature branches. Never commit directly to `main`.
- Make small logical commits.
- Merge shared contracts before dependent work.
- Pull/rebase shared changes before integration.
- Avoid destructive Git operations.

### Branches

- Naming: `feat/<topic>`, `fix/<topic>`, `docs/<topic>`, `test/<topic>`, `chore/<topic>`, `refactor/<topic>`, `perf/<topic>`, `contract/<topic>`.
- One branch per feature, for example `feat/session-replay-buffer`, `feat/ssh-keepalive`, `feat/directory-leases`.
- Branches are short-lived. Merge and delete them once finished.
- Contract changes (protocol messages, `Backend`, `SessionDirectory`, `SessionState`) get their own `contract/<topic>` branch and merge to `main` before any dependent branch continues.

### Commits

- Use Conventional Commits: `type(scope): summary`. Scope is the crate or area: `vt`, `pty`, `theme`, `proto`, `session`, `ssh`, `directory`, `server`, `web`, `desktop`, `log`, `obs`, `deploy`, `ci`, `docs`.
- Summary is imperative, lowercase, under 72 characters.
- Add a body when the reason is not obvious, especially for protocol and failure-handling decisions. Cite the RFC or document being followed.
- Commit frequently. Make at least one commit for each of:
    - a new file or crate
    - a new public type, trait, or message
    - a new failure-handling behavior
    - a new test file or chaos scenario
    - a new config option
    - a docs update
- Keep a commit under roughly 150 changed lines and one concern.
- Do not mix refactoring with feature work. Do not mix formatting changes with logic changes.
- Every commit must pass `cargo check --workspace`. Do not leave broken intermediate commits.
- Commit tests with their feature or immediately after, as `test(scope): ...`.
- Never commit `target/`, `node_modules/`, `.env`, credentials, private keys, or machine-specific paths.

### Integration

```
git switch main && git pull --ff-only
git switch -c feat/session-replay-buffer

# work in small steps, commit after each logical change

git fetch origin
git rebase origin/main
cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo test --workspace
(cd web && npm run lint && npm test)

git push -u origin feat/session-replay-buffer
# open a PR; merge with a merge commit (no squash, history is the progress log)
```

- Before opening a PR, rebase on the latest `main`.
- Merge with merge commits. Do not squash; the commit trail is part of the deliverable.
- Tag each finished phase: `phase-9-workspace`, `phase-10-server`, `phase-11-reattach`, `phase-12-ssh`, `phase-13-resilience`, `phase-14-security`, `phase-15-sharing`, `phase-16-multinode`, `phase-17-release`.

### Destructive operations

Do not run these without explicit human approval:

- `git push --force` (on your own feature branch after a rebase, `--force-with-lease` is the only acceptable form)
- `git reset --hard`
- `git clean -fd`
- `git checkout .` or `git restore .` on uncommitted work
- `git branch -D` on an unmerged branch
- `git rebase` or any history rewrite on `main` or on a branch others use
- `git commit --amend` on a commit that is already pushed

Also require approval before: dropping or migrating a database with real data, deleting stored credentials or host keys, and running chaos scripts against anything other than the local test environment.

If unsure, create a backup branch first: `git branch backup/<name>`.

## Coding Standards

- `cargo fmt` and `cargo clippy --workspace -- -D warnings` must pass.
- No `unwrap()` / `expect()` outside tests and startup code in `main`. Use `Result` with `thiserror` types in libraries and `anyhow` in binaries.
- No `unsafe` without a comment explaining why it is sound.
- Async rules: never block the runtime; every channel is bounded with a documented overflow policy; every network call has a timeout; document cancellation safety on any `select!` branch that owns state.
- Use `tracing`, not `println!`. Attach `session_id` and `node_id` to spans. Never log secrets, keystrokes, or session output.
- Keep modules under about 500 lines; split when they grow.
- Public items get doc comments. Failure-handling code gets a comment stating which row of the failure table it implements.
- Protocol and state-machine logic is covered by unit tests; recovery behavior is covered by chaos or integration tests in `tests/chaos/`.
- Frontend: TypeScript strict mode, no `any` in protocol code, lint and tests must pass.

Platforms: server on Linux (primary) and macOS; local PTY on Windows is best-effort; the SSH backend is cross-platform; the web client targets current evergreen browsers.

Performance targets:

- `seq 1 100000` through the browser completes without stalling the session
- idle sessions use near-zero CPU
- memory per session is bounded by configuration
- reattach from the replay buffer is fast enough to feel instant

## Agent Working Agreement

1. Identify the current phase from `docs/plan.md` and `git tag`. Do not skip phases or combine them.
2. Read the relevant docs and contracts before writing code. If a contract needs to change, make a `contract/*` branch first.
3. State the plan first: list the commits you intend to make for this task.
4. Implement the smallest slice, verify it, commit it, and repeat.
5. After each task run `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, and the web checks when `web/` changed. Then show `git status` and `git log --oneline -n 15`.
6. Tick the phase checklist in `docs/plan.md` as part of the relevant commit.
7. Never claim a guarantee that a test does not prove. When behavior is a limitation, write it in `docs/guarantees.md` instead of hiding it.
8. For every recovery feature, write the failure-injection test before calling it done.
9. At the end of every phase write `docs/learn/phase-N.md` in plain language: what was built, the key concepts (leases, fencing, backpressure, replay, host-key verification, and so on), why each design choice was made, what to read next, and where in the code to look. The human will study these later.
10. If blocked, record the blocker in `docs/integration.md` and ask. Do not silently work around a contract.
11. Ask before adding a dependency outside the tech stack table.
12. Follow `SKILLS.md` for routine permissions. Pushing, publishing, handling real secrets, and other external side effects still need explicit approval.
13. Do not add features to the frozen desktop app.

## Golden Demo

The full definition lives in `docs/runbooks/demo.md`. The project is demo-ready when all of this works from `docker compose up`:

1. Open the web app and log in over TLS.
2. Create a local session (loopback mode): a bash prompt appears with correct colors; `vim` and `htop` work; resizing reflows.
3. Create an SSH session to the compose-provided sshd. The first connection shows a host-key prompt; accepting it connects.
4. Run `seq 1 100000`, close the tab mid-run, reopen: the screen is identical with no gaps or duplicates.
5. Run the chaos script that drops the SSH connection: the UI shows `Reconnecting`, then resumes inside the same tmux session with state intact.
6. Repeat step 5 on a session without tmux: the UI shows a clear `Failed` state.
7. Open the same session in a second browser as a viewer: output mirrors, only the writer can type, and a stalled viewer does not slow the writer.
8. With two gateway nodes running: kill the node that owns an SSH+tmux session; the other node adopts it and the client resumes.
9. The metrics endpoint shows active sessions, reconnect counts, and replay lag.

## Definition of Done

- All phases 9 to 17 are tagged and merged to `main`.
- CI runs workspace tests plus containerized integration and chaos tests, and they pass.
- The golden demo passes from `docker compose up`.
- `docs/guarantees.md` matches the failure table and every row is backed by a test.
- `docs/threat-model.md` exists and every mandatory security item has a test or a documented reason.
- `README.md` explains the architecture, security model, deployment, and the honest limits (what is and is not guaranteed).
- Every phase has a `docs/learn/phase-N.md`.
- The Git history shows small, logical commits for every stage, crate, and feature.