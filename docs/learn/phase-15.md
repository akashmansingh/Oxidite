# Phase 15: Shared Sessions

## Summary
Phase 15 adds real-time multi-client collaboration to Oxidite sessions. Multiple browser clients can attach to the same session simultaneously: exactly one client acts as the single active Writer, while any number of additional clients attach as Viewers. All connected viewers receive the live terminal stream and converge to an identical screen state.

## Key Invariants & Architectural Rules
1. **Single Writer**: Only the active writer is allowed to submit terminal keystrokes and window resizes to the shell backend. Keystrokes sent by viewers are rejected/ignored.
2. **Writer Takeover**: Any authorized participant can request writer takeover, transitioning their mode to Writer and demoting the previous writer to Viewer, accompanied by a presence notification.
3. **Presence Notifications**: The session actor broadcasts `ServerMsg::Presence { session, writer_id, viewers_count }` to all connected clients whenever clients attach, detach, or change roles.
4. **Independent Ack Windows**: Every attached client maintains its own acknowledgment offset and unacked byte window.
5. **Slow Viewers Never Block Writers**: Flow control backpressure for reading the shell backend checks only the active writer's unacked bytes. A stalled or lagging viewer is marked as stalled and resynced from a fresh ANSI snapshot without stalling the shell or delaying the writer.
6. **Scoped Share Links**: Share tokens (`share_<uuid>`) are created via `/api/sessions/:id/share` with a configurable TTL, role (Viewer or Writer), and instant revocation capability via `DELETE /api/shares/:token`.

## Verification
- `crates/oxidite-session/tests/shared_sessions.rs`:
  - `test_multiple_clients_converge_to_identical_screen`: 1 writer and 2 viewers verify identical visual cell and attribute reproduction.
  - `test_stalled_viewer_does_not_delay_writer`: A viewer that ceases sending ACKs during a 2 MiB stream does not impede backend reads or the writer.
  - `test_single_writer_enforcement_and_takeover`: Verifies that viewer input is dropped until takeover succeeds, after which the new writer's input is processed.
- `crates/oxidite-server/src/share.rs`: Tests share token generation, TTL expiration, and revocation.
