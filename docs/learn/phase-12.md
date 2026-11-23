# Phase 12: SSH Backend & Host-Key Verification

## What Was Built

Phase 12 implemented the remote SSH backend contracts and trust-on-first-use (TOFU) security mechanisms:

1. **SSH Backend (`oxidite-ssh`):**
   - Implemented `SshBackend` fulfilling the `Backend` trait with channel write, resize forwarding, and clean teardown.
   - Designed configuration for password, public-key, and interactive authentication.

2. **Known Hosts & TOFU Store:**
   - `KnownHostsStore` tracks host key fingerprints per target host.
   - First connection prompts the user via `ServerMsg::HostKeyPrompt` with `HostKeyStatus::Unknown`.
   - On explicit acceptance via `ClientMsg::HostKeyDecision { accept: true }`, fingerprint is recorded.
   - Any changed host key generates `HostKeyStatus::Mismatch` and immediately terminates connection attempts to prevent man-in-the-middle attacks.

3. **Web Client Integration:**
   - Modal prompt dialog in `web/src/main.ts` intercepts `HostKeyPrompt`, displays host and fingerprint, and returns decision to server.

## Verification

- Automated integration tests in `crates/oxidite-server/tests/websocket_session.rs` verify:
  - Unknown host triggers `HostKeyPrompt` with host details.
  - Acceptance creates the session and advances to `ServerMsg::Created`.
  - Rejection produces `ErrorCode::Unauthorized`.
- Unit tests in `crates/oxidite-ssh/src/lib.rs` verify host key trust and mismatch detection.
