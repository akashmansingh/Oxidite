# Wire Protocol Specification

This document defines the WebSocket wire protocol between the Oxidite browser client and the `oxidited` session server.

## Overview

- **Transport:** Binary WebSocket frames over `ws://` (loopback / testing) or `wss://` (production with TLS).
- **Encoding:** MessagePack, encoded with named fields (`rmp_serde::to_vec_named`).
- **Endianness / Types:** Native MessagePack integers and binary buffers (`bin` format, via `serde_bytes`).
- **Sequencing:** Each byte transmitted on the session output stream is indexed by a monotonically increasing 64-bit byte `Offset`.

## Protocol Handshake

1. Upon opening the WebSocket connection to `/ws`, the browser must send `ClientMsg::Hello`:
   ```rust
   ClientMsg::Hello {
       protocol_version: 1,
       token: "<token>",
   }
   ```
   The `token` field accepts either an opaque device token (`od1.<device-id>.<secret>`), an authenticated session token, or a one-time enrollment grant.
2. The server verifies `protocol_version == 1` and authenticates the token in constant time. Malformed tokens are rejected generically without leaking device existence.
3. If successful, the server replies with `ServerMsg::Welcome`:
   ```rust
   ServerMsg::Welcome {
       server_version: "0.1.0",
   }
   ```
4. If invalid, the server responds with `ServerMsg::Error` (code `ProtocolMismatch` or `Unauthorized`) and closes the connection.

## Client Messages (`ClientMsg`)

| Variant | Body | Description |
|---|---|---|
| `Hello` | `{ protocol_version: u16, token: String }` | First message sent to authenticate. |
| `Create` | `{ backend: BackendSpec, size: TermSize }` | Request creation of a new persistent session. |
| `Attach` | `{ session: SessionId, last_offset: Option<Offset>, mode: AttachMode }` | Attach to an existing session with resume offset. |
| `Input` | `bytes` (MessagePack binary) | Raw user input bytes (keystrokes, paste). |
| `Resize` | `{ rows: u16, cols: u16 }` | Window / terminal dimension change. |
| `Ack` | `{ offset: Offset }` | Client confirms consumption of bytes up to `offset`. |
| `HostKeyDecision` | `{ accept: bool }` | User response to unknown SSH host key prompt. |
| `AuthResponse` | `{ answers: Vec<String> }` | Answers to keyboard-interactive prompts. |
| `Ping` | `{ nonce: u64 }` | Heartbeat keepalive ping. |
| `Detach` | unit | Detach client; session continues running in background. |
| `Kill` | unit | Terminate the shell process and destroy the session. |

## Server Messages (`ServerMsg`)

| Variant | Body | Description |
|---|---|---|
| `Welcome` | `{ server_version: String }` | Acknowledgment of valid `Hello`. |
| `Created` | `{ session: SessionId }` | Confirms session creation. |
| `Attached` | `{ session: SessionId, mode: AttachMode, start: ReplayStart }` | Confirms attachment. |
| `Snapshot` | `{ offset: Offset, bytes: Vec<u8> }` | Full screen state repaint escape sequences. |
| `Output` | `{ offset: Offset, bytes: Vec<u8> }` | Output stream chunk beginning at `offset`. |
| `State` | `{ session: SessionId, state: SessionState }` | Session lifecycle state change. |
| `HostKeyPrompt` | `{ host: String, fingerprint: String, known: HostKeyStatus }` | Prompt user to trust unknown host key. |
| `AuthPrompt` | `{ prompts: Vec<String> }` | Prompts for credentials (e.g. 2FA, OTP). |
| `Title` | `String` | Terminal window title update (OSC 0/2). |
| `Exit` | `{ code: Option<i32> }` | Shell process exited. |
| `Pong` | `{ nonce: u64 }` | Heartbeat keepalive pong response. |
| `Error` | `{ code: ErrorCode, message: String }` | Error notification. |

## Replay and Snapshot Semantics

When a client attaches with `Attach { session, last_offset, mode }`:
- If `last_offset` is `Some(offset)` and `offset >= buffer_tail && offset <= buffer_head`, the server replays missing stream chunks via `ReplayStart::Replay { from: offset }` and `Output` messages.
- If `last_offset` is `None` or `offset < buffer_tail` (data aged out), the server emits `ReplayStart::Snapshot { at: head }` followed by `Snapshot { offset: head, bytes }`.
- `Snapshot` bytes are standard ANSI escape sequences (`CSI 2J`, cursor moves, SGR color/style codes, character writes, and cursor visibility restoration). The client feeds them directly into xterm.js via `term.write()`.

## Flow Control Watermarks

- **High Watermark:** 1 MiB unacknowledged bytes. When `unacked_bytes > 1 MiB`, backend reads pause.
- **Low Watermark:** 256 KiB. When the client sends `Ack { offset }` reducing unacked bytes below 256 KiB, backend reads resume.
