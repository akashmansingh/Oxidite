# Golden Demo Runbook: Oxidite Platform

This runbook guides evaluators and engineers through the complete Oxidite platform demonstration from initial launch through failure injection, live collaboration, and node failover.

---

## 1. Quick Start

Ensure Docker and Docker Compose are installed, then boot the multi-node cluster:

```bash
docker compose -f deploy/docker-compose.yml up -d
```

Verify services are running and healthy:
```bash
docker compose -f deploy/docker-compose.yml ps
```

- **Gateway 1**: `http://localhost:3000` (Node: `gateway-node-1`)
- **Gateway 2**: `http://localhost:3001` (Node: `gateway-node-2`)
- **Directory**: PostgreSQL on `localhost:5432`
- **Target Host**: OpenSSH + tmux on `localhost:2222` (`developer` / `hunter2`)

---

## 2. Interactive Shell Demonstration

1. Open `http://localhost:3000` in Google Chrome or Firefox.
2. Enter the gateway access token: `dev-secret-token-12345`.
3. Click **New Terminal** and select **SSH**:
   - Host: `test-sshd` (or `localhost`)
   - Port: `2222`
   - User: `developer`
   - Password: `hunter2`
   - Multiplexer: `tmux`
4. Accept the Trust-On-First-Use (TOFU) host-key prompt.
5. Launch an interactive program:
   ```bash
   top
   ```
6. Resize your browser window. Verify that the remote PTY receives `window-change` signals and fits the terminal grid immediately without visual corruption.

---

## 3. Disconnect and Reattach Demonstration

1. Run an output counter:
   ```bash
   seq 1 100000
   ```
2. While numbers are rapidly scrolling, **close the browser tab completely**.
3. Observe on the server (`docker logs oxidite-gateway-1`) that the session state transitions to `Detached`, but the shell process and tmux session remain running at full speed.
4. Re-open `http://localhost:3000`.
5. Click on the active session in the sidebar.
6. The client sends `Attach` with `last_offset`. The session actor restores the exact screen via ANSI repaint snapshot and missing replay chunks. Notice:
   - **Zero gaps** in output.
   - **Zero duplicated lines**.
   - Identical cursor coordinates and attributes.

---

## 4. Multi-User Collaboration & Sharing Demonstration

1. In the first browser window (Writer), click **Share**.
2. Select **Viewer** mode and copy the generated link (`share_...`).
3. Open a second, incognito browser window and paste the link.
4. The incognito window attaches as a **Viewer**:
   - Both windows display the live terminal stream in real time.
   - Presence indicator in both windows updates to `1 active writer, 1 viewer`.
   - Any typing attempt in the Viewer window is dropped/ignored (read-only).
5. In the Viewer window, click **Takeover Writer**.
   - Presence updates immediately: incognito window becomes Writer; original window becomes Viewer.
   - Keystrokes in the second window now drive the terminal.

---

## 5. Gateway Node Crash & Transparent Failover

1. While an SSH+tmux session is active on Gateway 1 (`http://localhost:3000`), crash Gateway 1:
   ```bash
   docker stop oxidite-gateway-1
   ```
2. The browser connection to Gateway 1 drops.
3. Gateway 2 (`http://localhost:3001`) detects the orphaned session as its TTL lease expires in PostgreSQL.
4. Gateway 2 adopts the session with an incremented fencing token (`fencing_token = 2`), reconnects to the containerized sshd, and reattaches to `tmux new-session -A -s oxidite-<id>`.
5. Open `http://localhost:3001` in your browser. Attach to the session.
6. The terminal repaints the live shell state exactly where you left off.
7. Verify that if Gateway 1 is restarted (`docker start oxidite-gateway-1`), its stale lease and old fencing token (1) are rejected, preventing split-brain writes.

---

## 6. Observability & Health Verification

Inspect live Prometheus metrics:
```bash
curl -s http://localhost:3001/metrics
```

Verify readiness endpoint:
```bash
curl -i http://localhost:3001/ready
```
Returns `HTTP/1.1 200 OK` with body `READY`.
