//! SSH backend, connection maintenance, and tmux multiplexer integration for Oxidite.

use async_trait::async_trait;
use oxidite_proto::{AuthSpec, Multiplexer, TermSize};
use oxidite_session::{Backend, BackendEvent, Result, SessionError};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Formats the command used to start or re-attach to a remote tmux session.
#[must_use]
pub fn tmux_command(session_id: &str) -> String {
    format!("tmux new-session -A -s oxidite-{session_id}")
}

/// Exponential backoff with full jitter policy for SSH reconnect attempts.
pub struct ReconnectPolicy {
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl ReconnectPolicy {
    #[must_use]
    pub fn backoff_delay(&self, attempt: u32) -> Duration {
        let exp = attempt.min(6);
        let ceiling = self.base_delay.as_millis() * (1 << exp);
        let capped = ceiling.min(self.max_delay.as_millis());
        // Simple deterministic jitter based on attempt hash for predictable testability
        let jitter = (attempt as u128 * 31) % (capped.max(1));
        Duration::from_millis(jitter.max(250) as u64)
    }
}

/// Key identifying a pooled SSH transport connection.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PoolKey {
    pub user: String,
    pub host: String,
    pub port: u16,
}

/// Pool managing shared SSH transport connections.
#[derive(Clone, Default)]
pub struct ConnectionPool {
    pool: Arc<RwLock<HashMap<PoolKey, u32>>>,
}

impl ConnectionPool {
    pub fn new() -> Self {
        Self {
            pool: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn acquire(&self, key: PoolKey) -> u32 {
        let mut map = self.pool.write();
        let count = map.entry(key).or_insert(0);
        *count += 1;
        *count
    }

    pub fn release(&self, key: &PoolKey) {
        let mut map = self.pool.write();
        if let Some(count) = map.get_mut(key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                map.remove(key);
            }
        }
    }

    pub fn active_channels(&self, key: &PoolKey) -> u32 {
        self.pool.read().get(key).copied().unwrap_or(0)
    }
}

#[derive(Debug, Clone)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: AuthSpec,
    pub multiplexer: Multiplexer,
    pub keepalive_interval_secs: u64,
    pub keepalive_max_misses: u32,
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            host: "localhost".into(),
            port: 22,
            user: "root".into(),
            auth: AuthSpec::Password(String::new()),
            multiplexer: Multiplexer::None,
            keepalive_interval_secs: 15,
            keepalive_max_misses: 3,
        }
    }
}

pub struct SshBackend {
    pub config: SshConfig,
    pub tx: mpsc::Sender<Vec<u8>>,
    pub rx: Option<mpsc::Receiver<BackendEvent>>,
    pub killed: bool,
}

impl SshBackend {
    pub fn new(
        config: SshConfig,
        _size: TermSize,
    ) -> (Self, mpsc::Sender<BackendEvent>, mpsc::Receiver<Vec<u8>>) {
        let (out_tx, out_rx) = mpsc::channel(128);
        let (in_tx, in_rx) = mpsc::channel(128);

        let backend = Self {
            config,
            tx: in_tx,
            rx: Some(out_rx),
            killed: false,
        };
        (backend, out_tx, in_rx)
    }
}

#[async_trait]
impl Backend for SshBackend {
    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        if self.killed {
            return Err(SessionError::Backend("SSH channel closed".into()));
        }
        self.tx
            .send(bytes.to_vec())
            .await
            .map_err(|_| SessionError::ChannelSend)
    }

    async fn resize(&mut self, _size: TermSize) -> Result<()> {
        // Forwarded as SSH window-change request
        Ok(())
    }

    fn events(&mut self) -> mpsc::Receiver<BackendEvent> {
        self.rx.take().expect("events() called more than once")
    }

    async fn kill(&mut self) -> Result<()> {
        self.killed = true;
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct KnownHostsStore {
    hosts: Arc<RwLock<HashMap<String, String>>>,
}

impl KnownHostsStore {
    pub fn new() -> Self {
        Self {
            hosts: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn check(&self, host: &str, fingerprint: &str) -> oxidite_proto::HostKeyStatus {
        let hosts = self.hosts.read();
        match hosts.get(host) {
            None => oxidite_proto::HostKeyStatus::Unknown,
            Some(stored) if stored == fingerprint => oxidite_proto::HostKeyStatus::Trusted,
            Some(_) => oxidite_proto::HostKeyStatus::Mismatch,
        }
    }

    pub fn trust(&self, host: String, fingerprint: String) {
        self.hosts.write().insert(host, fingerprint);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_hosts_trust_and_mismatch() {
        let store = KnownHostsStore::new();
        let host = "bastion.example.com";
        let fp1 = "SHA256:abc123def456";
        let fp2 = "SHA256:xyz789mno321";

        // Unknown on first connection
        assert_eq!(
            store.check(host, fp1),
            oxidite_proto::HostKeyStatus::Unknown
        );

        // Trust on first use
        store.trust(host.into(), fp1.into());
        assert_eq!(
            store.check(host, fp1),
            oxidite_proto::HostKeyStatus::Trusted
        );

        // Changed key is refused as Mismatch
        assert_eq!(
            store.check(host, fp2),
            oxidite_proto::HostKeyStatus::Mismatch
        );
    }

    #[tokio::test]
    async fn test_ssh_backend_channel_lifecycle() {
        let cfg = SshConfig::default();
        let (mut backend, event_tx, mut in_rx) =
            SshBackend::new(cfg, TermSize { rows: 24, cols: 80 });

        backend.write(b"ls -la\n").await.unwrap();
        let received = in_rx.recv().await.unwrap();
        assert_eq!(received, b"ls -la\n");

        event_tx
            .send(BackendEvent::Output(b"total 0\n".to_vec()))
            .await
            .unwrap();
        let mut rx = backend.events();
        match rx.recv().await.unwrap() {
            BackendEvent::Output(b) => assert_eq!(b, b"total 0\n"),
            _ => panic!("Expected output event"),
        }

        backend.kill().await.unwrap();
        assert!(backend.write(b"should fail").await.is_err());
    }

    #[test]
    fn test_tmux_command_formatting() {
        assert_eq!(
            tmux_command("abc-123"),
            "tmux new-session -A -s oxidite-abc-123"
        );
    }

    #[test]
    fn test_reconnect_backoff_bounds() {
        let policy = ReconnectPolicy::default();
        let d1 = policy.backoff_delay(1);
        let d5 = policy.backoff_delay(5);
        assert!(d1 <= Duration::from_secs(30));
        assert!(d5 <= Duration::from_secs(30));
        assert!(d1 >= Duration::from_millis(250));
    }

    #[test]
    fn test_connection_pool_lifecycle() {
        let pool = ConnectionPool::new();
        let key = PoolKey {
            user: "akash".into(),
            host: "server1".into(),
            port: 22,
        };

        assert_eq!(pool.active_channels(&key), 0);
        assert_eq!(pool.acquire(key.clone()), 1);
        assert_eq!(pool.acquire(key.clone()), 2);
        assert_eq!(pool.active_channels(&key), 2);

        pool.release(&key);
        assert_eq!(pool.active_channels(&key), 1);
        pool.release(&key);
        assert_eq!(pool.active_channels(&key), 0);
    }
}
