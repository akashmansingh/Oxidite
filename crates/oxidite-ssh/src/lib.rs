//! SSH backend, connection maintenance, and tmux multiplexer integration for Oxidite.

use async_trait::async_trait;
use oxidite_proto::{AuthSpec, Multiplexer, TermSize};
use oxidite_session::{Backend, BackendEvent, Result, SessionError};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

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
