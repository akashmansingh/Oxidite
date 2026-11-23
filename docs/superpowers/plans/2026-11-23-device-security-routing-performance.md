# Device Security, Routing, and Performance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` task-by-task. Steps use checkbox syntax for tracking.

**Goal:** Add device-scoped authentication with local PIN unlock, safe PowerShell selection, explicit cache/routing policy, and real-traffic benchmark evidence.

**Architecture:** A device principal is established from a high-entropy credential, while Web Crypto uses the local PIN only to unwrap that credential from IndexedDB. Server route classification centralizes cache policy and redacted measurements; workers run fixed scenarios against a live server and retain bounded NDJSON evidence.

**Tech Stack:** Rust 2024, Tokio, Axum, Argon2id, SQLite via `sqlx` after compatibility verification, TypeScript, Web Crypto, IndexedDB, Vite, MessagePack.

**Spec:** `docs/superpowers/specs/2026-11-23-device-security-routing-performance-design.md`

## Global Constraints

- Remain on `feat/device-security-routing`; do not add desktop features or elevate PowerShell permissions.
- Keep LocalPty loopback-only unless explicitly configured; browser input never controls an executable path.
- Bound channels, artifacts, retention, and metric labels. Never log/persist plaintext PINs, credentials, tokens, terminal input, or output.
- Preserve binary MessagePack, the Hello-first handshake, per-user ownership, and honest 30-second cross-node revocation bound.
- Write each behavior test-first; pin/version-check new dependencies and commit both locks.

## Review Focus

- Malformed device credentials must reject generically without disclosing an ID (Task 3).
- Active cross-node devices must revalidate within 30 seconds after revocation (Task 5).
- Artifacts/metrics/audit output must never contain secrets or terminal output (Tasks 2, 3, and 7).
- Traversal-like static paths must never get immutable caching or another file's body (Task 1).
- Missing IndexedDB must not fall back to unencrypted browser storage (Task 4).

### Task 1: Classify routes and apply safe cache policy

**Files:** Create `crates/oxidite-server/src/routing.rs`; modify `crates/oxidite-server/src/lib.rs`; test `crates/oxidite-server/tests/routing.rs`.

**Interfaces:** `RouteClass`, `classify_path(&Path) -> RouteClass`, and `cache_control(RouteClass) -> HeaderValue`; `static_handler` classifies the resolved file before opening it.

- [x] Write tests for fingerprinted static immutable caching, HTML no-cache, API/metrics no-store, and traversal rejection.
- [x] Run `cargo test -p oxidite-server --test routing`; confirm RED.
- [x] Implement route classification and headers with resolved paths only.
- [x] Re-run focused test; confirm GREEN.
- [x] Commit `feat(server): classify routes and apply safe cache policy`.

### Task 2: Record redacted route and WebSocket measurements

**Files:** Modify `crates/oxidite-server/src/metrics.rs`, `crates/oxidite-server/src/lib.rs`; test `crates/oxidite-server/tests/observability.rs`.

**Interfaces:** `MetricsCollector::observe_http(RouteClass, StatusCode, Duration, u64)` and `observe_websocket_handshake(Duration)`.

- [x] Write metric tests proving route-class counters contain no identifiers, paths, or body values.
- [x] Run `cargo test -p oxidite-server --test observability`; confirm RED.
- [x] Add bounded counters/timers at HTTP/WS boundaries only.
- [x] Re-run focused test; confirm GREEN.
- [x] Commit `feat(obs): record redacted route and websocket measurements`.

### Task 3: Add durable device enrollment and authentication

**Files:** Create `crates/oxidite-server/src/devices.rs`; modify `auth.rs`, `lib.rs`, `Cargo.toml`, `Cargo.lock`, `docs/contracts/wire-protocol.md`, `docs/threat-model.md`; test `crates/oxidite-server/tests/device_auth.rs`.

**Interfaces:** `DeviceStore::{issue_enrollment_grant,enroll,authenticate,revoke,list_for_user}` and `Principal { user_id, device_id, role }`; accepted token format `od1.<uuid>.<secret>`.

- [x] Verify compatible pinned `sqlx` SQLite version; write RED tests for independent devices, generic malformed rejection, owner isolation, revocation, and redacted audit data.
- [x] Run `cargo test -p oxidite-server --test device_auth`; confirm RED.
- [x] Implement additive SQLite records, Argon2id hashes, one-use grants, and principal authorization; remove static development-token bypasses in device mode.
- [x] Update contracts; rerun focused test; confirm GREEN.
- [x] Commit `feat(server): add durable device credentials and enrollment`.

### Task 4: Build browser vault and PIN unlock

**Files:** Create `web/src/device-vault.ts`; modify `web/src/main.ts`, `connection.ts`, `protocol.ts`, `style.css`, `index.html`; test `web/src/device-vault.test.ts`.

**Interfaces:** `DeviceVault.enroll`, `.unlock`, `.forget` backed only by IndexedDB/Web Crypto.

- [ ] Write RED tests for encrypted storage, correct/wrong PIN, and unavailable IndexedDB without fallback.
- [ ] Run focused web test; confirm RED.
- [ ] Implement random salt/IV, PBKDF2-SHA-256, AES-GCM, and accessible enrollment/unlock UI.
- [ ] Run focused test plus `bun run lint`/`bun run build`; confirm GREEN.
- [ ] Commit `feat(web): unlock enrolled devices with a local pin`.

### Task 5: Enforce active device revocation

**Files:** Create `crates/oxidite-server/src/device_connections.rs`; modify `lib.rs`, `security.rs`, `docs/guarantees.md`; test `crates/oxidite-server/tests/device_revocation.rs`.

**Interfaces:** `DeviceConnectionRegistry::{register,unregister,close_local,requires_revalidation}`.

- [ ] Write RED integration tests for immediate local close and remote-node revalidation by 30 seconds.
- [ ] Run `cargo test -p oxidite-server --test device_revocation`; confirm RED.
- [ ] Implement bounded registry, close signals, and cancellation-safe 30-second checks.
- [ ] Re-run focused test; document the proven guarantee.
- [ ] Commit `feat(server): enforce device revocation on active connections`.

### Task 6: Expose safe system PowerShell choice

**Files:** Modify `crates/oxidite-pty/src/lib.rs`, `crates/oxidite-server/src/lib.rs`, `web/src/main.ts`, `web/index.html`, `web/src/protocol.ts`; test PTY module and `crates/oxidite-server/tests/powershell.rs`.

**Interfaces:** `ShellCapability { id, available }`; authenticated `GET /api/runtime/shells`; server-only mapping of `SystemPowerShell`.

- [ ] Write RED tests for present/absent PowerShell and rejection of arbitrary browser paths.
- [ ] Run focused PTY/server tests; confirm RED.
- [ ] Implement capability endpoint and UI while retaining LocalPty policy.
- [ ] Re-run focused tests and web checks; confirm GREEN.
- [ ] Commit `feat(pty): expose safe system powershell selection`.

### Task 7: Capture real-traffic benchmark evidence

**Files:** Create `crates/oxidite-server/tests/benchmark_evidence.rs`, `tests/benchmarks/run.ps1`, `tests/benchmarks/README.md`; modify `.gitignore`, demo/chaos runbooks.

**Interfaces:** bounded NDJSON fields `scenario_id`, opaque `trace_id`, route class, status, byte/timing/cache decisions, and redacted WS counters.

- [ ] Write RED tests for cold/warm assets, API no-store, terminal-flow evidence, and secret/output absence.
- [ ] Run focused benchmark test; confirm RED.
- [ ] Implement fixed scenario planner and PowerShell worker against a local live server with capped files/records.
- [ ] Run it; inspect evidence against real HTTP/WS traffic.
- [ ] Commit `test(server): capture bounded real-traffic benchmark evidence`.

### Task 8: Document and verify

**Files:** Modify `README.md`, `ARCHITECTURE.md`, `docs/guarantees.md`, `docs/threat-model.md`, `docs/integration.md`; create `docs/learn/phase-18.md`.

- [ ] Document only test-proven PIN, revocation, PowerShell, cache, and benchmark limits.
- [ ] Run `cargo fmt --check`; `cargo clippy --workspace -- -D warnings`; `cargo test --workspace`; `cd web; bun run lint; bun run build`.
- [ ] Resolve failures and commit `docs: document device security and performance guarantees`.
