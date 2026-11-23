# Chaos Engineering and Failure Injection Runbook

This runbook defines the failure injection procedures to verify Oxidite's resilience guarantees.

## Test 1: SSH Connection Severance with Tmux Persistence

### Goal
Verify that when the TCP stream between the gateway and target SSH host is dropped mid-stream, a session running inside tmux transitions to `Reconnecting`, re-establishes connection, and resumes the exact shell screen via snapshot.

### Procedure
1. Launch `oxidited` on gateway node.
2. Connect from browser client to target host with `tmux: true`.
3. In terminal, start an ongoing interactive program (e.g. `htop` or counter script).
4. Inject network severance using `iptables` or killing the TCP socket:
   ```bash
   ss -K dst <target-ip> dport 22
   ```
5. Observe the browser UI indicators:
   - Chain indicator transitions to `retry` ("Reconnecting to host").
   - Reconnect loop attempts backoff with full jitter.
6. Restore network path.
7. Verify postconditions:
   - Session reattaches to `tmux new-session -A -s oxidite-<id>`.
   - Screen snapshot repaints cleanly without corrupted escape codes.

---

## Test 2: SSH Connection Severance without Multiplexer

### Goal
Verify that when an SSH connection drops without a remote multiplexer, Oxidite marks the session `Failed` immediately and never leaves the UI in a silent hang.

### Procedure
1. Start an SSH session with `multiplexer: None`.
2. Terminate the TCP connection abruptly.
3. Verify postconditions:
   - Session transitions directly to `SessionState::Failed { reason: "..." }`.
   - UI status bar indicates `Failed` with reason code.
   - User is presented with clear options to start a new session.

---

## Test 3: Directory Lease Partition & Fencing Tokens

### Goal
Verify that a network partition between a gateway node and the session directory prevents split-brain writes.

### Procedure
1. Create session on Node A (holding lease token `T=1`).
2. Block Node A's directory renewal traffic for `> TTL`.
3. Node B adopts the session and issues fencing token `T=2`.
4. Restore Node A's network.
5. Attempt a write from Node A carrying token `T=1`.
6. Verify postconditions:
   - Directory returns `DirectoryError::FencingMismatch`.
   - Node A terminates ownership and client writes are rejected.
