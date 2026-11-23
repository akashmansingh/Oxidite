# Phase 11: Detach, Reattach, and Flow Control

## What Was Built

Phase 11 delivered persistent sessions surviving client disconnections and robust flow control under slow consumers:

1. **Cumulative Output Stream Offsets:**
   - Monotonic 64-bit byte offset (`Offset`) tracking every byte written by the shell.
   - Replay ring buffer (`ReplayBuffer`) with default 8 MiB bounded capacity.

2. **Idempotent Attach & Snapshots:**
   - Attaching with `last_offset` inside the buffer triggers zero-copy byte replay (`ReplayStart::Replay`).
   - Attaching with an expired or missing offset falls back to an ANSI repaint snapshot (`ReplayStart::Snapshot`).
   - Snapshot serialization encodes the terminal grid into ANSI sequences (`\x1b[2J`, cursor placement, SGR color/style attributes) ensuring standard VT client compatibility.

3. **Watermark Flow Control & Backpressure:**
   - High watermark (1 MiB): Backend reads are suspended when unacknowledged bytes exceed this threshold.
   - Low watermark (256 KiB): As client emits `Ack { offset }`, unacknowledged bytes decrease; backend reads resume below low watermark.
   - Idle timeout: Sessions detached for more than the configured duration (default 1 hour) are cleaned up gracefully.

## Verification

- Automated tests in `crates/oxidite-session/tests/attach_replay.rs` verify:
  - Exact slice replay within buffer.
  - Snapshot equivalence where repainted grid equals original grid down to individual cell attributes and coordinates.
