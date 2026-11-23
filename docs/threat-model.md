# Oxidite Threat Model

A browser-accessible terminal platform is inherently an endpoint capable of executing arbitrary code. Security must be architectural, defense-in-depth, and verified by automated tests.

---

## 1. Assets

1. **Gateway Server Infrastructure**: Memory, CPU, network sockets, file system access, and local shell execution environment.
2. **Target SSH Hosts**: Remote hosts accessed through SSH credentials, keys, and authorized sessions.
3. **User Credentials & Secrets**: Plaintext passwords, SSH private keys, bearer tokens, and session identifiers.
4. **Active Session Streams**: Keystrokes, terminal output (which may contain confidential data, environment variables, or private source code), and replay ring buffers.
5. **State Directory & Leases**: Session directory entries, node ownership records, and fencing tokens.

---

## 2. Threat Actors & Attack Vectors

| Attacker | Motivation / Capability | Attack Vector | Mitigation |
|---|---|---|---|
| **Network Eavesdropper** | Intercept or tamper with transit traffic | Man-in-the-Middle on WebSocket or SSH | TLS required on non-loopback; rustls cipher suites; strict host-key verification |
| **Malicious Web Page** | Execute commands via user's browser | Cross-Site WebSocket Hijacking (CSWSH) | Validate `Origin` header on HTTP upgrade; reject untrusted origins; strict CSP |
| **Unauthenticated Client** | Gain unauthorized terminal access | Brute force login or forge tokens | Argon2id password hashing; rate limiting; cryptographically random rotating tokens |
| **Malicious Authenticated User** | Hijack or observe other users' shells | Cross-user session attach, kill, or list | Strict per-user ownership check on all actor interactions; isolated session IDs |
| **Exhaustion / DoS Attacker** | Crash gateway or consume memory | Oversized frames, endless replay buffers, session flooding | Max message size (64 KiB); max sessions per user; ring buffer capacity cap (8 MiB) |
| **Compromised Log Aggregator** | Exfiltrate secrets from server logs | Inspecting application or gateway logs | Invariant: Never log keystrokes, passwords, private keys, tokens, or output bytes |
| **Untrusted Remote Network** | Pivot into internal gateway host | Local shell escape via `LocalPty` | `LocalPty` backend disabled on non-loopback binds unless explicitly allowed |

---

## 3. Mandatory Security Controls

### 3.1 Transport Layer Security (TLS)
- Non-loopback binds require TLS.
- Handled via `rustls` with modern TLS 1.2/1.3 cipher suites and ALPN (`h2`, `http/1.1`).

### 3.2 Cross-Site WebSocket Hijacking (CSWSH) Prevention
- On `/ws` HTTP upgrade, the server checks the `Origin` header against `SecurityConfig::allowed_origins`.
- Disallowed origins receive `403 Forbidden` and a security violation is recorded in the audit log.

### 3.3 Authentication & Password Hashing
- User credentials authenticated with Argon2id (`argon2` crate) using cryptographically secure random salts.
- Short-lived rotating session tokens issued upon login with configurable TTL.
- Optional OIDC bearer token verification for enterprise SSO.

### 3.4 Per-User Authorization & Isolation
- Every session is bound to an owner (`user_id`).
- `Attach`, `Kill`, and `list` operations enforce that the calling user matches the owner (or has `admin` role). Unauthorized attempts return `ErrorCode::Unauthorized`.

### 3.5 Local PTY Protection
- The `LocalPty` backend is strictly disabled when binding to external interfaces unless explicitly enabled by `allow_non_loopback_pty: true`. Multi-tenant gateways should connect only via SSH or containerized shells.

### 3.6 Rate Limiting & Resource Caps
- Rate limiter enforces sliding-window request caps on login and WebSocket handshakes per remote IP.
- Input frames capped at 64 KiB.
- Replay buffers capped at 8 MiB per session.
- Per-user session limits enforced on `Create`.

### 3.7 Audit Logging Invariant
- All access events (`login`, `session_create`, `session_attach`, `session_terminate`, `security_violation`) are recorded through `AuditLogger`.
- Keystrokes, passwords, terminal output bytes, tokens, and private keys NEVER reach logs or tracing subscribers.
