# Oxidite Gateway Real-Traffic Benchmark Harness

This harness collects bounded, size-capped NDJSON evidence of HTTP routing, cache policies, and WebSocket handshake measurements against a live Oxidite server.

## Purpose

The benchmark verifies:
1. **Route Classification and Cache Decisions**:
   - `RouteClass::Html`: application shell served with `no-cache`.
   - `RouteClass::FingerprintedAsset`: content-hashed assets served with `public, max-age=31536000, immutable`.
   - `RouteClass::Api` and `RouteClass::Metrics`: authenticated APIs and metrics served with `no-store`.
   - Traversal rejection: directory traversal paths return `400 Bad Request` without leaking file contents.
2. **Safe PowerShell Runtime Detection**:
   - `GET /api/runtime/shells` probe returns minimal capability objects without revealing local `PATH` or environment details.
3. **Redaction & Privacy Invariants**:
   - Benchmark artifacts never contain user IDs, device credentials (`od1.*`), tokens, passwords, local PINs, or terminal output.
4. **Bounded Evidence Sink**:
   - Output files are capped at 512 KiB and 100 records to prevent disk exhaustion.

## Usage

### Automated Integration Test (Rust)

Run the end-to-end scenario test in `crates/oxidite-server`:

```powershell
cargo test -p oxidite-server --test benchmark_evidence
```

### Live PowerShell Worker

Run against a local running server (`http://127.0.0.1:3000`):

```powershell
.\tests\benchmarks\run.ps1 -Url "http://127.0.0.1:3000" -Token "<admin-token>"
```

### Parameters

- `-Url`: Target gateway HTTP address (default `http://127.0.0.1:3000`).
- `-Token`: Optional bootstrap access token for testing authenticated API routes.
- `-OutputDir`: Directory for evidence output (default `tests/benchmarks/evidence/`).
- `-MaxRecords`: Maximum number of records to retain (default `100`).
- `-MaxBytes`: Maximum file size in bytes (default `524288` / 512 KiB).

## Artifact Schema

Each line of `benchmark-latest.ndjson` is an NDJSON object:

```json
{
  "scenario_id": "api_no_store_sessions",
  "trace_id": "90e1f3be-065b-4395-9bfd-aa14c330c6a8",
  "route_class": "api",
  "status": 200,
  "cache_decision": "no-store",
  "bytes_sent": 120,
  "bytes_received": 54,
  "duration_ms": 1.45,
  "ws_counters": null
}
```
