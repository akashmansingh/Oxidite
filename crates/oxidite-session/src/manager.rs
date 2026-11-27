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
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct SessionHandle {
    pub id: SessionId,
    pub tx: mpsc::Sender<ActorCmd>,
}

#[derive(Clone)]
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<SessionId, SessionHandle>>>,
    cwds: Arc<RwLock<HashMap<SessionId, std::path::PathBuf>>>,
    idle_timeout: Duration,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            cwds: Arc::new(RwLock::new(HashMap::new())),
            idle_timeout: super::actor::DEFAULT_IDLE_TIMEOUT,
        }
    }

    #[must_use]
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }

    fn make_exit_hook(&self) -> super::actor::ExitHook {
        let sessions = self.sessions.clone();
        let cwds = self.cwds.clone();
        Arc::new(move |id| {
            sessions.write().remove(&id);
            cwds.write().remove(&id);
        })
    }

    pub fn set_cwd(&self, id: SessionId, cwd: std::path::PathBuf) {
        self.cwds.write().insert(id, cwd);
    }

    pub fn get_cwd(&self, id: SessionId) -> Option<std::path::PathBuf> {
        self.cwds.read().get(&id).cloned()
    }

    pub fn remove_cwd(&self, id: SessionId) {
        self.cwds.write().remove(&id);
    }

    pub async fn create(&self, spec: BackendSpec, size: TermSize) -> Result<SessionId> {
        let id = SessionId::new_v4();
        let initial_cwd = match spec {
            BackendSpec::LocalPty { ref cwd, .. } => cwd.as_deref().map(std::path::PathBuf::from),
            BackendSpec::Ssh { .. } => None,
        };

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
        let actor = actor
            .with_initial_cwd(initial_cwd.clone())
            .with_idle_timeout(self.idle_timeout)
            .with_on_exit(self.make_exit_hook());

        tokio::spawn(actor.run());

        self.sessions.write().insert(id, SessionHandle { id, tx });
        if let Some(path) = initial_cwd {
            self.cwds.write().insert(id, path);
        }
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
        let actor = actor
            .with_idle_timeout(self.idle_timeout)
            .with_on_exit(self.make_exit_hook());

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
        self.cwds.write().remove(&id);
    }
}
