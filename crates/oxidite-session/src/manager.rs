//! Session manager for creating, listing, and accessing sessions.

use super::{
    Backend, Result, SessionError,
    actor::{ActorCmd, SessionActor},
    pty_backend::LocalPtyBackend,
};
use oxidite_proto::{BackendSpec, SessionId, TermSize};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct SessionHandle {
    pub id: SessionId,
    pub tx: mpsc::Sender<ActorCmd>,
}

#[derive(Clone, Default)]
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<SessionId, SessionHandle>>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn create(&self, spec: BackendSpec, size: TermSize) -> Result<SessionId> {
        let id = SessionId::new_v4();
        let backend: Box<dyn Backend> = match spec {
            BackendSpec::LocalPty { shell, cwd } => {
                Box::new(LocalPtyBackend::spawn(shell, cwd, size)?)
            }
            BackendSpec::Ssh { .. } => {
                return Err(SessionError::Backend(
                    "SSH backend initialized via oxidite-ssh".into(),
                ));
            }
        };

        let (tx, rx) = mpsc::channel(128);
        let (actor, _) = SessionActor::new(id, size, backend, rx);

        tokio::spawn(actor.run());

        self.sessions.write().insert(id, SessionHandle { id, tx });
        Ok(id)
    }

    pub fn create_with_backend(
        &self,
        id: SessionId,
        size: TermSize,
        backend: Box<dyn Backend>,
    ) -> SessionId {
        let (tx, rx) = mpsc::channel(128);
        let (actor, _) = SessionActor::new(id, size, backend, rx);

        tokio::spawn(actor.run());

        self.sessions.write().insert(id, SessionHandle { id, tx });
        id
    }

    pub fn insert_handle(&self, id: SessionId, handle: SessionHandle) {
        self.sessions.write().insert(id, handle);
    }

    pub fn get(&self, id: SessionId) -> Option<SessionHandle> {
        self.sessions.read().get(&id).cloned()
    }

    pub fn list(&self) -> Vec<SessionId> {
        self.sessions.read().keys().copied().collect()
    }

    pub fn remove(&self, id: SessionId) {
        self.sessions.write().remove(&id);
    }
}
