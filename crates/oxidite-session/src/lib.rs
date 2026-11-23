//! Session actor, replay buffer, and backend traits for Oxidite.

use async_trait::async_trait;
use oxidite_proto::{SessionId, SessionState, TermSize};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum BackendEvent {
    Output(Vec<u8>),
    State(SessionState),
    Exited(Option<i32>),
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Session not found: {0}")]
    NotFound(SessionId),
    #[error("Backend failure: {0}")]
    Backend(String),
    #[error("Channel send error")]
    ChannelSend,
    #[error("Replay buffer error: {0}")]
    Replay(String),
}

pub type Result<T> = std::result::Result<T, SessionError>;

#[async_trait]
pub trait Backend: Send {
    async fn write(&mut self, bytes: &[u8]) -> Result<()>;
    async fn resize(&mut self, size: TermSize) -> Result<()>;
    fn events(&mut self) -> mpsc::Receiver<BackendEvent>;
    async fn kill(&mut self) -> Result<()>;
}

pub mod actor;
pub mod manager;
pub mod pty_backend;
pub mod replay;
