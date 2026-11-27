//! Session actor owning terminal state, replay buffer, and backend.

use super::{Backend, BackendEvent, replay::ReplayBuffer};
use oxidite_proto::{
    AttachMode, Offset, ReplayStart, ServerMsg, SessionId, SessionState, TermSize,
};
use oxidite_vt::terminal::Terminal;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, oneshot};

pub const HIGH_WATERMARK: usize = 1024 * 1024; // 1 MiB
pub const LOW_WATERMARK: usize = 256 * 1024; // 256 KiB
/// Multi-hour default suspension threshold (6 hours) only triggered when zero clients are
/// attached AND no session activity has occurred for the full window.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(6 * 3600);
pub const WRITER_ACK_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(25);

pub type ExitHook = Arc<dyn Fn(SessionId) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct ClientInfo {
    pub client_id: u64,
    pub mode: AttachMode,
    pub last_acked_offset: Offset,
    pub unacked_bytes: usize,
    pub is_stalled: bool,
}

pub struct SessionActor {
    pub id: SessionId,
    pub terminal: Terminal,
    parser: oxidite_vt::parser::Parser,
    pub replay: ReplayBuffer,
    pub state: SessionState,
    pub backend: Box<dyn Backend>,
    pub receiver: mpsc::Receiver<ActorCmd>,
    pub client_tx: broadcast::Sender<ServerMsg>,
    pub active_writer: Option<u64>,
    pub clients: HashMap<u64, ClientInfo>,
    pub next_client_id: u64,
    pub writer_unacked_bytes: usize,
    pub high_watermark: usize,
    pub low_watermark: usize,
    pub detached_at: Option<Instant>,
    pub last_activity_at: Instant,
    stalled_since: Option<Instant>,
    pub idle_timeout: Duration,
    pub ack_watchdog_timeout: Duration,
    on_exit: Option<ExitHook>,
    pub active_cwd: Option<std::path::PathBuf>,
    last_os_cwd: Option<std::path::PathBuf>,
    last_osc_cwd: Option<std::path::PathBuf>,
}

pub enum ActorCmd {
    Attach {
        mode: AttachMode,
        last_offset: Option<Offset>,
        client_id: Option<u64>,
        resp: oneshot::Sender<AttachResult>,
    },
    TakeoverWriter {
        client_id: u64,
        resp: oneshot::Sender<bool>,
    },
    Input {
        client_id: Option<u64>,
        bytes: Vec<u8>,
    },
    Resize {
        size: TermSize,
    },
    Ack {
        client_id: Option<u64>,
        offset: Offset,
    },
    Detach {
        client_id: Option<u64>,
    },
    Kill,
    GetState {
        resp: oneshot::Sender<SessionState>,
    },
    GetActiveWriter {
        resp: oneshot::Sender<Option<u64>>,
    },
    GetPresence {
        resp: oneshot::Sender<(Option<u64>, usize)>,
    },
    GetCwd {
        resp: oneshot::Sender<Option<std::path::PathBuf>>,
    },
    GetAgentContext {
        resp: oneshot::Sender<(Option<u32>, String, Option<std::path::PathBuf>)>,
    },
}

pub struct AttachResult {
    pub client_id: u64,
    pub mode: AttachMode,
    pub start: ReplayStart,
    pub snapshot: Option<Vec<u8>>,
    pub replay_chunks: Vec<Vec<u8>>,
    pub current_offset: Offset,
    pub rx: broadcast::Receiver<ServerMsg>,
}

impl SessionActor {
    pub fn new(
        id: SessionId,
        size: TermSize,
        backend: Box<dyn Backend>,
        receiver: mpsc::Receiver<ActorCmd>,
    ) -> (Self, broadcast::Sender<ServerMsg>) {
        let (client_tx, _) = broadcast::channel(512);
        let actor = Self {
            id,
            terminal: Terminal::new(size.cols.into(), size.rows.into()),
            parser: oxidite_vt::parser::Parser::new(),
            replay: ReplayBuffer::new(8 * 1024 * 1024), // 8 MiB default
            state: SessionState::Starting,
            backend,
            receiver,
            client_tx: client_tx.clone(),
            active_writer: None,
            clients: HashMap::new(),
            next_client_id: 1,
            writer_unacked_bytes: 0,
            high_watermark: HIGH_WATERMARK,
            low_watermark: LOW_WATERMARK,
            detached_at: None,
            last_activity_at: Instant::now(),
            stalled_since: None,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            ack_watchdog_timeout: WRITER_ACK_WATCHDOG_TIMEOUT,
            on_exit: None,
            active_cwd: None,
            last_os_cwd: None,
            last_osc_cwd: None,
        };
        (actor, client_tx)
    }

    #[must_use]
    pub fn with_initial_cwd(mut self, cwd: Option<std::path::PathBuf>) -> Self {
        self.active_cwd = cwd;
        self
    }

    #[must_use]
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_on_exit(mut self, hook: ExitHook) -> Self {
        self.on_exit = Some(hook);
        self
    }

    fn update_stall_timer(&mut self) {
        if self.active_writer.is_some() && self.writer_unacked_bytes >= self.high_watermark {
            self.stalled_since.get_or_insert_with(Instant::now);
        } else {
            self.stalled_since = None;
        }
    }

    fn reset_writer_backpressure(&mut self) {
        self.writer_unacked_bytes = 0;
        self.stalled_since = None;
    }

    /// Synchronizes live working directory from either the OS child process or OSC sequences.
    fn sync_cwd(&mut self) -> Option<std::path::PathBuf> {
        let os_cwd = self.backend.current_cwd();
        if os_cwd.is_some() && os_cwd != self.last_os_cwd {
            self.last_os_cwd = os_cwd.clone();
            self.active_cwd = os_cwd;
        }
        let osc_cwd = self.terminal.cwd.clone();
        if osc_cwd.is_some() && osc_cwd != self.last_osc_cwd {
            self.last_osc_cwd = osc_cwd.clone();
            self.active_cwd = osc_cwd;
        }
        self.active_cwd.clone()
    }

    fn viewers_count(&self) -> usize {
        self.clients
            .values()
            .filter(|c| c.mode == AttachMode::Viewer)
            .count()
    }

    fn broadcast_presence(&self) {
        let _ = self.client_tx.send(ServerMsg::Presence {
            session: self.id,
            writer_id: self.active_writer.map(|id| id.to_string()),
            viewers_count: self.viewers_count(),
        });
    }

    pub async fn run(mut self) {
        let mut backend_events = self.backend.events();
        self.state = SessionState::Running;
        let _ = self.client_tx.send(ServerMsg::State {
            session: self.id,
            state: self.state.clone(),
        });

        let mut watchdog_tick = tokio::time::interval(Duration::from_secs(5));
        watchdog_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut idle_tick = tokio::time::interval(Duration::from_secs(60));
        idle_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = watchdog_tick.tick() => {
                    if let Some(since) = self.stalled_since
                        && self.active_writer.is_some()
                        && since.elapsed() >= self.ack_watchdog_timeout
                    {
                        if let Some(prev_cid) = self.active_writer.take()
                            && let Some(prev) = self.clients.get_mut(&prev_cid)
                        {
                            prev.mode = AttachMode::Viewer;
                            prev.is_stalled = true;
                        }
                        self.reset_writer_backpressure();
                        self.broadcast_presence();
                    }
                }
                _ = idle_tick.tick() => {
                    if self.clients.is_empty() && self.last_activity_at.elapsed() >= self.idle_timeout {
                        let _ = self.backend.kill().await;
                        self.state = SessionState::Exited { code: None };
                        let _ = self.client_tx.send(ServerMsg::Exit { code: None });
                        break;
                    }
                }
                Some(cmd) = self.receiver.recv() => {
                    match cmd {
                        ActorCmd::Attach { mode, last_offset, client_id, resp } => {
                            self.detached_at = None;
                            self.last_activity_at = Instant::now();
                            if matches!(self.state, SessionState::Detached) {
                                // Detached -> Running is a legal transition: a returning client makes the session live again.
                                self.state = SessionState::Running;
                                let _ = self.client_tx.send(ServerMsg::State {
                                    session: self.id,
                                    state: self.state.clone(),
                                });
                            }
                            let cid = client_id.unwrap_or_else(|| {
                                let id = self.next_client_id;
                                self.next_client_id += 1;
                                id
                            });

                            let actual_mode = match mode {
                                AttachMode::Writer => {
                                    // Reset backpressure when writer ownership changes hands so a stalled
                                    // prior writer never blocks the newly attached client.
                                    if let Some(prev_cid) = self.active_writer
                                        && prev_cid != cid
                                        && let Some(prev) = self.clients.get_mut(&prev_cid)
                                    {
                                        prev.mode = AttachMode::Viewer;
                                    }
                                    self.active_writer = Some(cid);
                                    self.reset_writer_backpressure();
                                    AttachMode::Writer
                                }
                                AttachMode::Viewer => AttachMode::Viewer,
                            };

                            let head = self.replay.head_offset();
                            let tail = self.replay.tail_offset();

                            self.clients.insert(
                                cid,
                                ClientInfo {
                                    client_id: cid,
                                    mode: actual_mode,
                                    last_acked_offset: last_offset.unwrap_or(head),
                                    unacked_bytes: 0,
                                    is_stalled: false,
                                },
                            );

                            self.broadcast_presence();

                            let attach_res = if let Some(last) = last_offset {
                                if last >= tail && last <= head {
                                    let chunk = self.replay.get_from(last).unwrap_or_default();
                                    AttachResult {
                                        client_id: cid,
                                        mode: actual_mode,
                                        start: ReplayStart::Replay { from: last },
                                        snapshot: None,
                                        replay_chunks: vec![chunk],
                                        current_offset: head,
                                        rx: self.client_tx.subscribe(),
                                    }
                                } else {
                                    let snapshot_bytes = oxidite_vt::snapshot::serialize_snapshot(&self.terminal);
                                    AttachResult {
                                        client_id: cid,
                                        mode: actual_mode,
                                        start: ReplayStart::Snapshot { at: head },
                                        snapshot: Some(snapshot_bytes),
                                        replay_chunks: Vec::new(),
                                        current_offset: head,
                                        rx: self.client_tx.subscribe(),
                                    }
                                }
                            } else {
                                let snapshot_bytes = oxidite_vt::snapshot::serialize_snapshot(&self.terminal);
                                AttachResult {
                                    client_id: cid,
                                    mode: actual_mode,
                                    start: ReplayStart::Snapshot { at: head },
                                    snapshot: Some(snapshot_bytes),
                                    replay_chunks: Vec::new(),
                                    current_offset: head,
                                    rx: self.client_tx.subscribe(),
                                }
                            };
                            let _ = resp.send(attach_res);
                        }
                        ActorCmd::TakeoverWriter { client_id, resp } => {
                            self.last_activity_at = Instant::now();
                            if let Some(prev_cid) = self.active_writer
                                && prev_cid != client_id
                                && let Some(prev) = self.clients.get_mut(&prev_cid)
                            {
                                prev.mode = AttachMode::Viewer;
                            }
                            if let Some(client) = self.clients.get_mut(&client_id) {
                                client.mode = AttachMode::Writer;
                                self.active_writer = Some(client_id);
                                self.reset_writer_backpressure();
                                self.broadcast_presence();
                                let _ = resp.send(true);
                            } else {
                                let _ = resp.send(false);
                            }
                        }
                        ActorCmd::Input { client_id, bytes } => {
                            self.last_activity_at = Instant::now();
                            // Enforce single-writer: only active writer may send input to backend
                            let allowed = match (client_id, self.active_writer) {
                                (None, _) => true, // direct/test input
                                (Some(cid), Some(w_id)) => cid == w_id,
                                (Some(_), None) => false,
                            };
                            if allowed {
                                let _ = self.backend.write(&bytes).await;
                            }
                        }
                        ActorCmd::Resize { size } => {
                            self.last_activity_at = Instant::now();
                            self.terminal.resize(size.cols.into(), size.rows.into());
                            let _ = self.backend.resize(size).await;
                        }
                        ActorCmd::Ack { client_id, offset } => {
                            let head = self.replay.head_offset();
                            if let Some(cid) = client_id {
                                if let Some(client) = self.clients.get_mut(&cid) {
                                    client.last_acked_offset = offset;
                                    client.unacked_bytes = (head.saturating_sub(offset)) as usize;
                                    if client.is_stalled && client.unacked_bytes < self.low_watermark {
                                        client.is_stalled = false;
                                    }
                                }
                                if self.active_writer == Some(cid) {
                                    self.writer_unacked_bytes = (head.saturating_sub(offset)) as usize;
                                    self.update_stall_timer();
                                }
                            } else {
                                // Default / legacy fallback
                                self.writer_unacked_bytes = (head.saturating_sub(offset)) as usize;
                                self.update_stall_timer();
                            }
                        }
                        ActorCmd::Detach { client_id } => {
                            self.last_activity_at = Instant::now();
                            if let Some(cid) = client_id {
                                self.clients.remove(&cid);
                                if self.active_writer == Some(cid) {
                                    // Unthrottle output immediately when the writer detaches so remaining viewers
                                    // and the background shell never stall waiting for acks from a gone client.
                                    self.active_writer = None;
                                    self.reset_writer_backpressure();
                                }
                            } else {
                                self.active_writer = None;
                                self.reset_writer_backpressure();
                            }
                            self.broadcast_presence();
                            if self.clients.is_empty() {
                                self.state = SessionState::Detached;
                                self.detached_at = Some(Instant::now());
                                let _ = self.client_tx.send(ServerMsg::State {
                                    session: self.id,
                                    state: self.state.clone(),
                                });
                            }
                        }
                        ActorCmd::Kill => {
                            let _ = self.backend.kill().await;
                            self.state = SessionState::Exited { code: None };
                            let _ = self.client_tx.send(ServerMsg::Exit { code: None });
                            break;
                        }
                        ActorCmd::GetState { resp } => {
                            let _ = resp.send(self.state.clone());
                        }
                        ActorCmd::GetActiveWriter { resp } => {
                            let _ = resp.send(self.active_writer);
                        }
                        ActorCmd::GetPresence { resp } => {
                            let _ = resp.send((self.active_writer, self.viewers_count()));
                        }
                        ActorCmd::GetCwd { resp } => {
                            let _ = resp.send(self.sync_cwd());
                        }
                        ActorCmd::GetAgentContext { resp } => {
                            let cwd = self.sync_cwd();
                            let _ = resp.send((
                                self.backend.shell_pid(),
                                self.terminal.title.clone(),
                                cwd,
                            ));
                        }
                    }
                }
                // Invariant: Flow control throttles ONLY when an active writer exceeds high_watermark.
                // Detached or viewer-only sessions drain freely without deadlocking background shells.
                Some(event) = backend_events.recv(), if self.active_writer.is_none() || self.writer_unacked_bytes < self.high_watermark => {
                    match event {
                        BackendEvent::Output(bytes) => {
                            self.last_activity_at = Instant::now();
                            // If ConPTY queries cursor position report (ESC [ 6 n),
                            // reply immediately so PowerShell does not block on startup!
                            if bytes.windows(4).any(|w| w == b"\x1b[6n") {
                                let _ = self.backend.write(b"\x1b[1;1R").await;
                            }

                            let start_offset = self.replay.head_offset();
                            self.replay.append(&bytes);
                            // Feed persistent VT parser so escape sequences split across reads stay intact.
                            self.parser.advance(&mut self.terminal, &bytes);
                            self.sync_cwd();

                            if self.active_writer.is_some() {
                                self.writer_unacked_bytes += bytes.len();
                                self.update_stall_timer();
                            }

                            // Update each client's unacked tracking
                            for client in self.clients.values_mut() {
                                client.unacked_bytes += bytes.len();
                                if client.mode == AttachMode::Viewer && client.unacked_bytes > self.high_watermark {
                                    // Mark slow viewer as stalled. It does not block the session!
                                    client.is_stalled = true;
                                }
                            }

                            let _ = self.client_tx.send(ServerMsg::Output {
                                offset: start_offset,
                                bytes,
                            });
                        }
                        BackendEvent::State(state) => {
                            self.state = state.clone();
                            let _ = self.client_tx.send(ServerMsg::State {
                                session: self.id,
                                state,
                            });
                        }
                        BackendEvent::Exited(code) => {
                            self.state = SessionState::Exited { code };
                            let _ = self.client_tx.send(ServerMsg::Exit { code });
                            break;
                        }
                    }
                }
                else => break,
            }
        }
        if let Some(hook) = self.on_exit.take() {
            hook(self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Result;
    use async_trait::async_trait;

    struct DummyBackend {
        _tx: mpsc::Sender<BackendEvent>,
        rx: Option<mpsc::Receiver<BackendEvent>>,
        killed: Arc<std::sync::atomic::AtomicBool>,
    }

    impl DummyBackend {
        fn new() -> (
            Self,
            mpsc::Sender<BackendEvent>,
            Arc<std::sync::atomic::AtomicBool>,
        ) {
            let (tx, rx) = mpsc::channel(64);
            let killed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            (
                Self {
                    _tx: tx.clone(),
                    rx: Some(rx),
                    killed: killed.clone(),
                },
                tx,
                killed,
            )
        }
    }

    #[async_trait]
    impl Backend for DummyBackend {
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
            self.killed.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn writer_handoff_and_detach_reset_backpressure() {
        let (backend, btx, _) = DummyBackend::new();
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let sid = SessionId::new_v4();
        let (mut actor, _) = SessionActor::new(
            sid,
            TermSize { rows: 24, cols: 80 },
            Box::new(backend),
            cmd_rx,
        );
        actor.high_watermark = 64;
        tokio::spawn(actor.run());

        // 1. Writer attaches and hits high watermark
        let (rtx, rrx) = oneshot::channel();
        cmd_tx
            .send(ActorCmd::Attach {
                mode: AttachMode::Writer,
                last_offset: None,
                client_id: Some(1),
                resp: rtx,
            })
            .await
            .unwrap();
        let _ = rrx.await.unwrap();

        btx.send(BackendEvent::Output(vec![b'a'; 128]))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;

        // 2. Detach active writer -> backpressure resets immediately and output drains
        cmd_tx
            .send(ActorCmd::Detach { client_id: Some(1) })
            .await
            .unwrap();
        btx.send(BackendEvent::Output(b"after_detach".to_vec()))
            .await
            .unwrap();

        let mut drained = false;
        let mut last_snap = String::new();
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(15)).await;
            let (vtx, vrx) = oneshot::channel();
            cmd_tx
                .send(ActorCmd::Attach {
                    mode: AttachMode::Viewer,
                    last_offset: None,
                    client_id: None,
                    resp: vtx,
                })
                .await
                .unwrap();
            let attached_v = vrx.await.unwrap();
            let snap = String::from_utf8_lossy(&attached_v.snapshot.unwrap()).into_owned();
            if snap.contains("after_detach") {
                drained = true;
                break;
            }
            last_snap = snap;
        }
        assert!(drained, "expected drained output, got {last_snap:?}");

        let (rtx2, rrx2) = oneshot::channel();
        cmd_tx
            .send(ActorCmd::Attach {
                mode: AttachMode::Writer,
                last_offset: None,
                client_id: Some(2),
                resp: rtx2,
            })
            .await
            .unwrap();
        let attached2 = rrx2.await.unwrap();
        let snap2 = String::from_utf8_lossy(&attached2.snapshot.unwrap()).into_owned();
        assert!(
            snap2.contains("after_detach"),
            "expected writer attach snapshot to preserve drained output, got {snap2:?}"
        );

        cmd_tx.send(ActorCmd::Kill).await.unwrap();
    }

    #[tokio::test]
    async fn writer_ack_watchdog_demotes_stalled_writer() {
        let (backend, btx, _) = DummyBackend::new();
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let sid = SessionId::new_v4();
        let (mut actor, _) = SessionActor::new(
            sid,
            TermSize { rows: 24, cols: 80 },
            Box::new(backend),
            cmd_rx,
        );
        actor.high_watermark = 16;
        actor.ack_watchdog_timeout = Duration::from_millis(50);
        tokio::spawn(actor.run());

        let (rtx, rrx) = oneshot::channel();
        cmd_tx
            .send(ActorCmd::Attach {
                mode: AttachMode::Writer,
                last_offset: None,
                client_id: Some(9),
                resp: rtx,
            })
            .await
            .unwrap();
        let _ = rrx.await.unwrap();

        btx.send(BackendEvent::Output(vec![b'x'; 32]))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(6)).await;

        let (wtx, wrx) = oneshot::channel();
        cmd_tx
            .send(ActorCmd::GetActiveWriter { resp: wtx })
            .await
            .unwrap();
        assert_eq!(wrx.await.unwrap(), None);

        cmd_tx.send(ActorCmd::Kill).await.unwrap();
    }

    #[tokio::test]
    async fn idle_suspension_triggers_only_after_inactivity_when_detached() {
        let (backend, _btx, killed) = DummyBackend::new();
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let sid = SessionId::new_v4();
        let exited = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let exited_hook = exited.clone();
        let (mut actor, _) = SessionActor::new(
            sid,
            TermSize { rows: 24, cols: 80 },
            Box::new(backend),
            cmd_rx,
        );
        actor.idle_timeout = Duration::from_secs(3600);
        let actor = actor.with_on_exit(Arc::new(move |_| {
            exited_hook.store(true, std::sync::atomic::Ordering::SeqCst);
        }));
        tokio::spawn(actor.run());

        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!killed.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!exited.load(std::sync::atomic::Ordering::SeqCst));

        cmd_tx.send(ActorCmd::Kill).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(exited.load(std::sync::atomic::Ordering::SeqCst));
    }
}
