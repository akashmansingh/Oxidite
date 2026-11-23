# Failure Semantics and Guarantees

This document records the exact system guarantees of Oxidite under every failure condition. Every guarantee is verifiable by automated tests.

## Failure Matrix

| Failure Mode | Immediate Behavior | Recovery Mechanism | Guarantees |
|---|---|---|---|
| **Browser tab closes / network drops** | Session enters `Detached` state. PTY and shell process remain running unhindered. | Browser reconnects and sends `Attach` with `last_offset`. | If `last_offset` is inside replay buffer (default 8 MiB), exact missing bytes are streamed. If outside buffer, full repaint `Snapshot` restores the visible screen without duplicates or gaps. |
| **Gateway-to-target TCP drops (with tmux)** | SSH channel is severed. Remote shell survives inside detached tmux session. | SSH connection manager marks state `Reconnecting`, backs off with full jitter, reconnects, reattaches to `tmux new-session -A -s oxidite-<id>`, and sends repaint `Snapshot`. | Zero loss of remote process state. Full screen redrawn accurately on reconnection. |
| **Gateway-to-target TCP drops (no tmux)** | Remote shell receives `SIGHUP` and terminates. | Session actor enters `Failed { reason }`. | Clean exit surfaced immediately to attached clients. No silent hangs. |
| **Gateway node crashes (SSH + tmux)** | TCP connection drops, in-memory state lost. Directory lease expires. | Peer gateway node adopts the orphan via lease stealing, re-establishes SSH + tmux session, generates new snapshot. | Remote processes persist; client resumes with transparent failover. |
| **Gateway node crashes (Local PTY)** | Shell process and operating system PTY handle terminate with host process. | Session marked `Failed`. | Documented platform limitation: local processes cannot outlive the host gateway operating system. |
| **Network partition between node and directory** | Gateway node fails to renew TTL lease before expiry. | Node refuses incoming writes carrying stale fencing tokens (`FencingMismatch`). | Split-brain prevention: exactly one node ever holds the active fencing token. |
| **Slow or stalled client** | Unacknowledged byte count exceeds high watermark (1 MiB). | Session actor applies backpressure to backend reader. Resumes once unacked bytes drop below low watermark (256 KiB). For viewer mode, slow viewers resync from a snapshot. | Bounded memory usage: server memory never grows unbounded regardless of client speed. |

## Invariants

1. **At-most-once Input:** Keystrokes and pastes are never replayed upon reconnect. Unsent input is dropped to prevent unintended command execution.
2. **Single Sequencer:** Output byte offsets are monotonically increasing 64-bit integers assigned solely by the owning session actor.
3. **Repaint Snapshots:** Snapshots are emitted as standard ANSI escape sequences, requiring no special client parsing logic.
