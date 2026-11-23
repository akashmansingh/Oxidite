# Phase 17: Durability, Observability, and Release

## Summary
Phase 17 delivers the final production release milestone for the Oxidite server platform. It adds an append-only segmented session durability engine, asciicast v2 session exports, Prometheus observability metrics, multi-stage Docker containerization, a multi-node cluster deployment environment, and complete operational runbooks.

## Key Deliverables
1. **Append-Only Segmented Session Log (`crates/oxidite-session/src/durability.rs`)**:
   - `SessionLog` tracks timestamped output segments per session.
   - Snapshot compaction replaces older segments with a terminal screen checkpoint, bounding disk and memory usage.
   - Replay API allows extracting arbitrary byte ranges `[from, to)`.
   - Asciicast v2 exporter converts recorded sessions into standard `.cast` JSONL format for terminal playback.
2. **Prometheus Metrics & Readiness (`crates/oxidite-server/src/metrics.rs`)**:
   - `/metrics` exposes Prometheus exposition format metrics: `oxidite_active_sessions`, `oxidite_reconnects_total`, `oxidite_bytes_transferred_total`, and `oxidite_replay_lag_bytes`.
   - `/ready` verifies directory connectivity, returning `200 OK` ("READY") or `503 Service Unavailable` if the node is partitioned.
   - `session_span` attaches structured tracing contexts (session id, backend kind) to asynchronous workflows.
3. **Containerized Deployment (`deploy/`)**:
   - `deploy/Dockerfile`: Multi-stage build compiling TypeScript/Vite web assets via Bun, building `oxidited` in Rust, and packaging into a minimal Debian runtime image.
   - `deploy/docker-compose.yml`: Spins up a clustered environment with two gateway nodes (`gateway-1`, `gateway-2`), a PostgreSQL directory store, and a containerized OpenSSH test server with tmux.
4. **Golden Demo Runbook (`docs/runbooks/demo.md`)**:
   - Step-by-step verification guide demonstrating interactive shells, disconnect/reattach convergence, multi-user sharing with writer takeover, gateway node crash failover, and metrics inspection.
5. **Documentation & Release Artifacts**:
   - Comprehensive `README.md` rewrite detailing architecture, failure semantics, crate boundaries, and deployment commands.

## Final Verification
- Full workspace tests pass cleanly:
  - `oxidite-vt`: Parser, grid, scrollback, and snapshot serialization tests.
  - `oxidite-session`: Replay buffer, attach/replay, multi-client sharing, and durability log tests.
  - `oxidite-server`: WebSockets, SSH TOFU host keys, Argon2 authentication, rate limiting, and Prometheus metrics tests.
  - `oxidite-directory`: Leases, TTL renewals, fencing tokens, and partition chaos tests.
- Zero clippy warnings with `cargo clippy --workspace -- -D warnings`.
- Code formatted with `cargo fmt --check`.
