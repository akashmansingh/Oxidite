# Phase 16: Multi-Node Ownership and Failover

## Summary
Phase 16 establishes multi-node clustering and high-availability session ownership for Oxidite gateways. In a distributed topology, multiple gateway nodes share a centralized or distributed session directory. Sessions are protected by time-to-live (TTL) leases and monotonically increasing fencing tokens to prevent split-brain execution across partitions.

## Key Architectural Invariants
1. **TTL Leases**: Each session is leased to a single gateway node for a bounded duration (e.g. 30–60 seconds). The owning node must renew its lease before expiry.
2. **Monotonically Increasing Fencing Tokens**: Every lease claim generates a monotonically incremented `fencing_token`. Any write or state transition carrying a stale token is rejected with `FencingMismatch`.
3. **Partition Isolation (No Split-Brain)**: If a gateway node is partitioned from the directory, its lease renewal fails. When the lease expires, peer nodes detect an orphan and adopt it with an incremented fencing token. If the partitioned node recovers, any stale writes it attempts are refused.
4. **Orphan Adoption**: When a gateway node crashes, peer nodes discover expired leases through `list_orphans()`. For SSH sessions backed by tmux (`tmux new-session -A -s oxidite-<id>`), the adopting node connects to the target host, attaches to the existing tmux session, and generates a fresh snapshot.
5. **Connection Redirection & Proxying**: Clients connecting to Node A for a session owned by Node B are greeted with Node B's identity, preventing split-state manipulation.

## Verification
- `tests/chaos/multi_node.rs`:
  - `test_partition_lease_renewals_and_verify_no_split_brain`: Injects a simulated network partition, verifies lease expiration, adoption by peer node with token 2, and rejection of stale token 1 writes.
  - `test_kill_owner_node_and_verify_orphan_adoption_and_client_resume`: Simulates node crash, verifies orphan detection, adoption by Node 2, and successful owner lookup.
- `crates/oxidite-directory/src/lib.rs`: Unit tests for lease creation, renewal, release, and `PartitionableDirectory` partition simulation.
