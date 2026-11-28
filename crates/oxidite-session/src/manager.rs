//! Session manager for creating, listing, and accessing sessions.

use super::{
    Backend, Result, SessionError,
    actor::{ActorCmd, SessionActor},
    pty_backend::LocalPtyBackend,
};
use oxidite_proto::{BackendSpec, SessionId, SessionState, TermSize};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const MAX_RETAINED_EXITED_SESSIONS: usize = 256;
const EXITED_SESSION_TTL: Duration = Duration::from_secs(300);

#[derive(Clone)]
pub struct SessionHandle {
    pub id: SessionId,
    pub tx: mpsc::Sender<ActorCmd>,
}

#[derive(Clone)]
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<SessionId, SessionHandle>>>,
    cwds: Arc<RwLock<HashMap<SessionId, std::path::PathBuf>>>,
    exited: Arc<RwLock<HashMap<SessionId, (SessionState, Instant)>>>,
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
            exited: Arc::new(RwLock::new(HashMap::new())),
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
        let exited = self.exited.clone();
        Arc::new(move |id, state| {
            sessions.write().remove(&id);
            cwds.write().remove(&id);
            if state.is_terminal() {
                let now = Instant::now();
                let mut ex = exited.write();
                ex.retain(|_, (_, at)| now.duration_since(*at) < EXITED_SESSION_TTL);
                if ex.len() >= MAX_RETAINED_EXITED_SESSIONS
                    && let Some(oldest_id) =
                        ex.iter().min_by_key(|(_, (_, at))| *at).map(|(k, _)| *k)
                {
                    ex.remove(&oldest_id);
                }
                ex.insert(id, (state, now));
            }
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

    pub fn get_exit_state(&self, id: SessionId) -> Option<SessionState> {
        let now = Instant::now();
        self.exited
            .read()
            .get(&id)
            .filter(|(_, at)| now.duration_since(*at) < EXITED_SESSION_TTL)
            .map(|(st, _)| st.clone())
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

        // Insert into maps BEFORE spawning so a fast exit hook never races ahead of insertion.
        self.sessions.write().insert(id, SessionHandle { id, tx });
        if let Some(path) = initial_cwd {
            self.cwds.write().insert(id, path);
        }
        tokio::spawn(actor.run());
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

        self.sessions.write().insert(id, SessionHandle { id, tx });
        tokio::spawn(actor.run());
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
        self.exited.write().remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BackendEvent;
    use async_trait::async_trait;

    struct ImmediateExitBackend {
        rx: Option<mpsc::Receiver<BackendEvent>>,
    }

    #[async_trait]
    impl Backend for ImmediateExitBackend {
        async fn write(&mut self, _bytes: &[u8]) -> Result<()> {
            Ok(())
        }
        async fn resize(&mut self, _size: TermSize) -> Result<()> {
            Ok(())
        }
        fn events(&mut self) -> mpsc::Receiver<BackendEvent> {
            self.rx.take().unwrap()
        }
        async fn kill(&mut self) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn fast_exiting_backend_leaves_no_dead_handle_and_records_exit_state() {
        let manager = SessionManager::new();
        for _ in 0..50 {
            let (tx, rx) = mpsc::channel(4);
            tx.send(BackendEvent::Exited(Some(42))).await.unwrap();
            let sid = SessionId::new_v4();
            manager.create_with_backend(
                sid,
                TermSize { rows: 24, cols: 80 },
                Box::new(ImmediateExitBackend { rx: Some(rx) }),
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
            assert!(
                manager.get(sid).is_none(),
                "dead handle remained in session map after fast exit"
            );
            assert_eq!(
                manager.get_exit_state(sid),
                Some(SessionState::Exited { code: Some(42) })
            );
        }
    }
}
