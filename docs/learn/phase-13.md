# Phase 13: Connection Maintenance & Resilience

## What Was Built

Phase 13 engineered the state machine and connection maintenance infrastructure to withstand real-world network instability:

1. **Explicit Session State Machine:**
   - Formal transition graph enforcing strict lifecycle paths (`Starting -> Running -> Degraded -> Reconnecting -> Running`).
   - Terminal states (`Exited`, `Failed`) forbid outgoing transitions, preventing ghost revivals.

2. **Keepalives & Degraded State Detection:**
   - Periodic keepalive probes (default interval 15 seconds, max misses 3).
   - When keepalive misses exceed the threshold, the session enters `Degraded`, signaling to the attached client that the link is unstable before full severance.

3. **Exponential Backoff & Full Jitter:**
   - `ReconnectPolicy` implements exponential backoff capped at 30 seconds with full random jitter, preventing thundering herd spikes when an SSH host recovers.

4. **Remote Multiplexer (tmux) Persistence:**
   - Sessions are launched in tmux via `tmux new-session -A -s oxidite-<session_id>`.
   - On reconnection, the channel re-attaches to the exact existing session.
   - Without tmux, connection severance receives a clear, honest `Failed` state rather than an indefinite freeze.

## Verification

- Automated chaos tests in `tests/chaos/connection_drop.rs` verify:
  - Transition flow to `Degraded` and `Reconnecting` with recovery under tmux.
  - Immediate `Failed` state and terminal safety when multiplexing is absent.
- Runbook in `docs/runbooks/chaos.md` specifies failure-injection reproduction steps.
