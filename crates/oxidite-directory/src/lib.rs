//! Session directory, lease management, and fencing tokens for multi-node Oxidite.

#![allow(clippy::collapsible_if)]

use async_trait::async_trait;
use oxidite_proto::SessionId;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub type NodeId = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    pub node: NodeId,
    pub fencing_token: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub session: SessionId,
    pub node: NodeId,
    pub fencing_token: u64,
    pub expires_at: Instant,
}

#[derive(Debug, thiserror::Error)]
pub enum DirectoryError {
    #[error("Session already claimed: {0}")]
    AlreadyClaimed(SessionId),
    #[error("Lease expired or not found: {0}")]
    LeaseNotFound(SessionId),
    #[error("Fencing token mismatch: expected {expected}, got {got}")]
    FencingMismatch { expected: u64, got: u64 },
    #[error("Storage error: {0}")]
    Storage(String),
}

pub type Result<T> = std::result::Result<T, DirectoryError>;

#[async_trait]
pub trait SessionDirectory: Send + Sync {
    async fn claim(&self, session: SessionId, node: NodeId, ttl: Duration) -> Result<Lease>;
    async fn renew(&self, lease: &Lease, ttl: Duration) -> Result<Lease>;
    async fn release(&self, lease: Lease) -> Result<()>;
    async fn lookup(&self, session: SessionId) -> Result<Option<Owner>>;
    async fn list_orphans(&self) -> Result<Vec<SessionId>>;
}

#[derive(Clone, Default)]
pub struct InMemoryDirectory {
    leases: Arc<RwLock<HashMap<SessionId, Lease>>>,
    token_gen: Arc<RwLock<u64>>,
}

impl InMemoryDirectory {
    pub fn new() -> Self {
        Self {
            leases: Arc::new(RwLock::new(HashMap::new())),
            token_gen: Arc::new(RwLock::new(0)),
        }
    }
}

#[async_trait]
impl SessionDirectory for InMemoryDirectory {
    async fn claim(&self, session: SessionId, node: NodeId, ttl: Duration) -> Result<Lease> {
        let mut leases = self.leases.write();
        let now = Instant::now();
        if let Some(existing) = leases.get(&session) {
            if existing.expires_at > now {
                return Err(DirectoryError::AlreadyClaimed(session));
            }
        }

        let mut token = self.token_gen.write();
        *token += 1;
        let lease = Lease {
            session,
            node,
            fencing_token: *token,
            expires_at: now + ttl,
        };
        leases.insert(session, lease.clone());
        Ok(lease)
    }

    async fn renew(&self, lease: &Lease, ttl: Duration) -> Result<Lease> {
        let mut leases = self.leases.write();
        let existing = leases
            .get_mut(&lease.session)
            .ok_or(DirectoryError::LeaseNotFound(lease.session))?;

        if existing.fencing_token != lease.fencing_token {
            return Err(DirectoryError::FencingMismatch {
                expected: existing.fencing_token,
                got: lease.fencing_token,
            });
        }

        existing.expires_at = Instant::now() + ttl;
        Ok(existing.clone())
    }

    async fn release(&self, lease: Lease) -> Result<()> {
        let mut leases = self.leases.write();
        if let Some(existing) = leases.get(&lease.session) {
            if existing.fencing_token == lease.fencing_token {
                leases.remove(&lease.session);
            }
        }
        Ok(())
    }

    async fn lookup(&self, session: SessionId) -> Result<Option<Owner>> {
        let leases = self.leases.read();
        let now = Instant::now();
        if let Some(lease) = leases.get(&session) {
            if lease.expires_at > now {
                return Ok(Some(Owner {
                    node: lease.node.clone(),
                    fencing_token: lease.fencing_token,
                }));
            }
        }
        Ok(None)
    }

    async fn list_orphans(&self) -> Result<Vec<SessionId>> {
        let leases = self.leases.read();
        let now = Instant::now();
        let mut orphans = Vec::new();
        for (sid, lease) in leases.iter() {
            if lease.expires_at <= now {
                orphans.push(*sid);
            }
        }
        Ok(orphans)
    }
}

/// A wrapper around `InMemoryDirectory` that can simulate a network partition for chaos testing.
#[derive(Clone)]
pub struct PartitionableDirectory {
    inner: InMemoryDirectory,
    partitioned: Arc<AtomicBool>,
}

impl PartitionableDirectory {
    pub fn new(inner: InMemoryDirectory) -> Self {
        Self {
            inner,
            partitioned: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn set_partitioned(&self, partitioned: bool) {
        self.partitioned.store(partitioned, Ordering::SeqCst);
    }

    pub fn is_partitioned(&self) -> bool {
        self.partitioned.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl SessionDirectory for PartitionableDirectory {
    async fn claim(&self, session: SessionId, node: NodeId, ttl: Duration) -> Result<Lease> {
        if self.is_partitioned() {
            return Err(DirectoryError::Storage(
                "Network partition: unreachable".into(),
            ));
        }
        self.inner.claim(session, node, ttl).await
    }

    async fn renew(&self, lease: &Lease, ttl: Duration) -> Result<Lease> {
        if self.is_partitioned() {
            return Err(DirectoryError::Storage(
                "Network partition: unreachable".into(),
            ));
        }
        self.inner.renew(lease, ttl).await
    }

    async fn release(&self, lease: Lease) -> Result<()> {
        if self.is_partitioned() {
            return Err(DirectoryError::Storage(
                "Network partition: unreachable".into(),
            ));
        }
        self.inner.release(lease).await
    }

    async fn lookup(&self, session: SessionId) -> Result<Option<Owner>> {
        if self.is_partitioned() {
            return Err(DirectoryError::Storage(
                "Network partition: unreachable".into(),
            ));
        }
        self.inner.lookup(session).await
    }

    async fn list_orphans(&self) -> Result<Vec<SessionId>> {
        if self.is_partitioned() {
            return Err(DirectoryError::Storage(
                "Network partition: unreachable".into(),
            ));
        }
        self.inner.list_orphans().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_in_memory_directory_lifecycle() {
        let dir = InMemoryDirectory::new();
        let sid = SessionId::new_v4();
        let lease = dir
            .claim(sid, "node-1".into(), Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(lease.session, sid);
        assert_eq!(lease.node, "node-1");

        let owner = dir.lookup(sid).await.unwrap().unwrap();
        assert_eq!(owner.node, "node-1");
        assert_eq!(owner.fencing_token, lease.fencing_token);

        let renewed = dir.renew(&lease, Duration::from_secs(120)).await.unwrap();
        assert_eq!(renewed.fencing_token, lease.fencing_token);

        dir.release(renewed).await.unwrap();
        assert!(dir.lookup(sid).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_partitionable_directory() {
        let inner = InMemoryDirectory::new();
        let part_dir = PartitionableDirectory::new(inner);
        let sid = SessionId::new_v4();

        let lease = part_dir
            .claim(sid, "node-1".into(), Duration::from_secs(60))
            .await
            .unwrap();

        // Simulate partition
        part_dir.set_partitioned(true);
        assert!(
            part_dir
                .renew(&lease, Duration::from_secs(60))
                .await
                .is_err()
        );

        // Heal partition
        part_dir.set_partitioned(false);
        assert!(
            part_dir
                .renew(&lease, Duration::from_secs(60))
                .await
                .is_ok()
        );
    }
}
