# Device-Scoped Security, Routing, and Performance Design

## Intent

Make Oxidite's browser terminal safer to use from multiple devices without
weakening the existing ownership and loopback protections. Add measurable,
bounded performance instrumentation for static assets, HTTP APIs, WebSocket
traffic, cache decisions, and local storage. Make the system PowerShell
experience explicit on Windows while preserving the rule that a local shell
runs with the gateway process's operating-system permissions.

The requested outcome is an operator who can identify and revoke a device,
trace a benchmark finding to the actual route and bytes that produced it, and
use the installed PowerShell through the existing ConPTY backend.

## Goals

- Enroll multiple named browser devices independently and revoke one without
  disrupting the others.
- Keep a PIN out of the network authentication path: it unlocks a device
  credential locally, while the server authenticates a high-entropy,
  device-specific credential.
- Bind every authenticated connection, audit record, and created session to a
  user and a device identity. Never include credential material, terminal
  input, or output in logs, metrics, benchmark artifacts, or error messages.
- Provide a Windows PowerShell choice that launches the system-installed
  `pwsh.exe` or `powershell.exe` through the existing PTY, with an accurate
  unavailable/error state.
- Add cache and routing policy that improves static asset delivery without
  caching API, WebSocket, terminal, or credential traffic.
- Run repeatable benchmark scenarios against real HTTP/WebSocket routes and
  record route, cache outcome, byte counts, durations, and opaque trace IDs.

## Non-Goals

- A PIN is not a replacement for server authentication, a password, MFA, or
  browser/OS key protection.
- No arbitrary command planner or worker service will be added to the gateway.
  Benchmark planning and workers are test/benchmark harness roles; their
  evidence is the real server traffic and generated measurement artifact.
- No bypass of `LocalPty` policy: non-loopback local shells remain disabled
  unless explicitly configured, and PowerShell inherits the gateway process's
  permissions.
- No cache of WebSocket frames, terminal output, credentials, authenticated
  responses, or private session metadata.
- No cross-node proxy redesign; existing ownership/lease handling remains the
  authority for session placement.

## Approaches Considered

### PIN as the network credential

The server would hash and verify a short PIN on every login. It is simple to
display, but a PIN has low entropy, needs aggressive online-throttling, and is
inferior to the existing random token against guessing and replay.

### Shared long-lived bearer token

Retaining one token across all devices is the least implementation work, but
it cannot identify or revoke one device and increases the blast radius of a
leak.

### Selected: enrolled device credential with local PIN unlock

An authenticated bootstrap flow creates a random device credential. The
server stores only an Argon2id hash, a device ID, an owner ID, lifecycle
timestamps, and a user-selected non-secret device label. The browser stores
the credential only in IndexedDB, encrypted with a Web Crypto key derived
from the local PIN and a random per-device salt. The PIN never crosses the
network or reaches the server. A device sends its device ID and decrypted
credential over TLS in the normal protocol `Hello` token field using a
versioned opaque format. The server verifies it, applies expiry/revocation,
and establishes the user/device principal for that connection.

This preserves the bearer token solely as a short-lived initial enrollment
grant (or an existing password login result), enables per-device revocation,
and avoids pretending that a low-entropy PIN is a strong remote credential.

## Authentication and Authorization Design

The approved bootstrap principal is a one-time, loopback-only local
administrator enrollment grant. It establishes the first user/device rather
than acting as a permanent shared `admin` identity. Later devices enroll only
through an authenticated user's short-lived, single-use enrollment grant.

### Enrollment

1. A user first authenticates with the existing loopback startup token or
   password login. The server issues a short-lived enrollment grant bound to
   that user; it is single-use and never persisted by the browser.
2. The browser generates no credential itself. The server generates at least
   256 bits using its CSPRNG, returns it once with a UUID device ID, and stores
   only its Argon2id hash.
3. The browser asks for a six-or-more-digit local PIN, derives an AES-GCM
   wrapping key via Web Crypto PBKDF2-SHA-256 with a random salt and a bounded,
   documented iteration count, and stores the encrypted credential, salt,
   IV, device ID, and non-secret device label in IndexedDB.
4. The server acknowledges enrollment without echoing the secret. The UI
   clears the enrollment grant and plaintext credential from memory as soon as
   the encrypted record is committed.

### Authentication and Device Identity

- `Hello.token` becomes a versioned opaque device token such as
  `od1.<device-id>.<secret>`; it remains a MessagePack string to avoid an
  unnecessary protocol version break. Its parser rejects malformed values in
  constant-time secret verification paths and returns the existing generic
  `Unauthorized` error.
- The principal carries `user_id`, `device_id`, and role. Every session
  create/list/attach/kill/share authorization check uses `user_id`; audit
  records also use `device_id`.
- Each session's owner remains a user, never a device, so an owner's other
  enrolled devices can resume it. Viewer share links retain their constrained
  scope and do not receive an owner/device principal.
- The server records `last_seen_at`, expiry, and revoked status. It supports
  list-my-devices and revoke-my-device endpoints; revocation invalidates new
  handshakes immediately and closes connections held by the local node
  immediately. Every node revalidates device status at most every 30 seconds;
  that is the cross-node revocation bound until a shared notification channel
  is explicitly designed and tested.
- A device cannot list, revoke, attach to, kill, or create a share for another
  user's resources. Errors are generic and do not disclose whether an ID
  exists.

### Data Handling

- Device secrets, enrollment grants, raw bearer tokens, PINs, Web Crypto
  material, SSH credentials, and terminal bytes are redacted at every logging
  and metrics boundary.
- Device records and session ownership move from process-local maps to the
  existing stack's SQLite-first durable store. The implementation checks the
  current compatible `sqlx` release before pinning it. Schema migrations use
  additive tables and indexes only; no existing credentials are deleted.
- The user-selected label is display-only, length-limited, and never used as
  an authorization key. It is not emitted to Prometheus labels.

## PowerShell Design

The existing PTY default already chooses `pwsh.exe`, then `powershell.exe`,
then `cmd.exe`, and configures `TERM`, `COLORTERM`, `TERM_PROGRAM`, and
`WT_SESSION`. The change exposes this capability safely:

- Server-side capability detection returns only approved shell choices and
  availability, never PATH contents or arbitrary executable locations.
- The browser can select `System PowerShell`, which maps to the detected
  executable rather than accepting a browser-supplied path. An unavailable
  PowerShell option explains the fallback.
- The explicit option follows the same loopback/non-loopback `LocalPty`
  guard, owner checks, bounded backend channels, and audit policy as every
  local shell. It does not elevate permissions or import profiles/secrets.

## Routing and Cache Policy

Route policy is centralized rather than scattered through handlers:

| Route class | Examples | Cache-Control | Evidence recorded |
|---|---|---|---|
| Fingerprinted static asset | JS, CSS, fonts, images with a content hash | `public, max-age=31536000, immutable` | route class, status, size, cache decision |
| HTML shell | `/`, SPA fallback | `no-cache` | route class, status, size |
| API/auth/device endpoints | `/api/*` | `no-store` | route template, status, duration only |
| WebSocket | `/ws` | no HTTP response caching | handshake status and frame byte counters |
| Metrics/health | `/metrics`, `/health`, `/ready` | `no-store` | status and duration only |

Cache eligibility is based on the resolved static asset and content hash, not
user-provided paths. The static file handler rejects traversal and attaches
`Vary`/security headers consistently. API responses avoid shared caches and
no cache key includes a credential, session ID, device ID, or terminal data.

The development Vite proxy remains development-only. Production routing stays
same-origin so the browser never has to choose an untrusted gateway route.

## Measurements and Benchmark Evidence

The benchmark harness has a planner that creates a fixed, reviewed scenario
matrix and workers that execute real HTTP requests and WebSocket sessions
against a started `oxidited`. It is not a production autonomous agent.

Each worker writes bounded NDJSON measurement records with:

- scenario ID and opaque trace ID;
- route template/class and cache decision;
- HTTP status, request/response byte counts, and elapsed time;
- WebSocket handshake time, output byte count, acknowledgement lag, and
  reconnect count;
- storage record size/count and enrollment/unlock timing, never encrypted
  plaintext or credentials.

Scenario coverage includes cold/warm fingerprinted asset fetches, HTML and
API no-store behavior, rejected traversal/cache attempts, device enrollment,
PIN unlock, device revocation, a PowerShell capability query, and a terminal
output flow. Evidence files are ignored by Git, capped in size and retention,
and point to the route/trace data that proves each finding. Prometheus gains
low-cardinality counters/histograms only; it never labels by session, user,
device, token, URL query, or raw path.

Performance gates are baseline-first rather than machine-specific latency
claims: a warm fetch of a fingerprinted asset must be eligible for immutable
caching and transfer no response body when the browser cache satisfies it;
HTML/API/device endpoints must remain non-cacheable. The harness retains
before/after byte and latency measurements for comparison without failing on
an arbitrary millisecond number.

## Failure Semantics

- Bad or locked local PIN: the browser cannot unwrap the local credential;
  no network request is attempted and no server authentication state changes.
- Revoked/expired device: subsequent handshakes fail generically; the active
  connection is closed and its session detaches rather than dies.
- IndexedDB unavailable or corrupt: the device is treated as unenrolled and
  must use the normal bootstrap/enrollment flow; no credential fallback is
  stored in localStorage or sessionStorage.
- Cache miss or invalid asset: the static handler serves a real response or
  fails normally; it never serves another user's/API data.
- Benchmark worker failure: its artifact identifies the scenario and route,
  but a benchmark does not change routing, cache policy, device state, or
  session ownership.

## Acceptance Criteria

- A Windows host with PowerShell installed can select the system PowerShell
  and receives a live PTY; a host without it presents an honest unavailable
  state. External `LocalPty` policy remains enforced.
- Two devices enrolled to the same user receive distinct IDs and can both
  access only that user's sessions. Revoking one immediately prevents only
  that device from establishing or retaining a connection.
- PIN bytes never appear in HTTP/WebSocket payloads, audit output, server
  storage, or browser unencrypted storage.
- Static assets receive the correct immutable/no-cache policy by route class;
  API, HTML, WebSocket, and credential responses are never cacheable.
- A benchmark run produces bounded evidence tied to actual endpoint and
  WebSocket traffic, and tests prove it contains no secret or terminal data.

## Builder Decisions Left Open

- Exact SQLite schema layout, migration framework details, and `sqlx` version
  after compatibility verification.
- Exact PBKDF2 iteration calibration, which will target a documented local
  unlock latency and be tested as a minimum threshold rather than a device-
  specific constant.
- Histogram bucket values after a baseline benchmark on the supported hosts.
