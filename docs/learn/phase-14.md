# Phase 14: Authentication and Security Hardening

## Summary
Phase 14 transitions Oxidite from a developer prototype with a static loopback bearer token into a hardened gateway server suitable for production deployments. Because a web terminal directly exposes shell execution capabilities, every security control specified in the threat model is implemented and backed by automated tests.

## Key Changes
1. **Rustls Configuration**: Added TLS server setup using `tokio-rustls` and `rustls-pki-types`. Enforced the rule that non-loopback binds require TLS.
2. **Origin Checking**: Enforced an `Origin` allowlist during WebSocket HTTP upgrade, preventing Cross-Site WebSocket Hijacking (CSWSH).
3. **Argon2id Password Hashing**: Implemented cryptographically secure password hashing and verification using `argon2` v0.5 with random salts.
4. **User Store & Rotating Session Tokens**: Created `UserStore` with user registration, credential authentication, and short-lived rotating session tokens.
5. **Per-User Ownership Checks**: Associated each created session with an owner `user_id`. Verified ownership on `Attach`, `Kill`, and session listing, preventing cross-user session interception.
6. **Rate Limiting**: Added sliding-window `RateLimiter` protecting login and connection endpoints from brute-force attempts and denial of service.
7. **Audit Logging**: Added `AuditLogger` with strict data-hygiene invariants: keystrokes, passwords, output bytes, tokens, and private keys are never logged.
8. **Resource Caps & LocalPty Hardening**: Enforced message size limits (64 KiB), maximum sessions per user, and default prohibition of `LocalPty` on non-loopback binds.
9. **Security Headers & CSP**: Added HTTP middleware injecting `Content-Security-Policy`, `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, and `Referrer-Policy: strict-origin-when-cross-origin`.
10. **Threat Model**: Documented assets, threat actors, attack vectors, and mitigations in `docs/threat-model.md`.

## Verification
- Unit and integration tests in `crates/oxidite-server/tests/security.rs` verify:
  - Argon2 password hashing and verification.
  - Rate limiting sliding windows.
  - WebSocket origin allowlist rejection of untrusted origins.
  - Per-user ownership enforcement (preventing User B from attaching to User A's session).
- All existing workspace tests pass with zero clippy warnings.
