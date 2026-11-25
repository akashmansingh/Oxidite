//! Session actor owning terminal state, replay buffer, and backend.

use super::{Backend, BackendEvent, replay::ReplayBuffer};
use oxidite_proto::{
    AttachMode, Offset, ReplayStart, ServerMsg, SessionId, SessionState, TermSize,
};
use oxidite_vt::terminal::Terminal;
use std::collections::HashMap;
use tokio::sync::{broadcast, mpsc, oneshot};

pub const HIGH_WATERMARK: usize = 1024 * 1024; // 1 MiB
pub const LOW_WATERMARK: usize = 256 * 1024; // 256 KiB
pub const DEFAULT_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3600);

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
    pub detached_at: Option<std::time::Instant>,
    pub idle_timeout: std::time::Duration,
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
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
        };
        (actor, client_tx)
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

        loop {
            tokio::select! {
                Some(cmd) = self.receiver.recv() => {
                    match cmd {
                        ActorCmd::Attach { mode, last_offset, client_id, resp } => {
                            self.detached_at = None;
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
                                    // Single writer rule: if there's no writer, claim it.
                                    // If there is already an active writer, takeover or assign viewer.
                                    self.active_writer = Some(cid);
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
                            if let Some(client) = self.clients.get_mut(&client_id) {
                                client.mode = AttachMode::Writer;
                                self.active_writer = Some(client_id);
                                self.broadcast_presence();
                                let _ = resp.send(true);
                            } else {
                                let _ = resp.send(false);
                            }
                        }
                        ActorCmd::Input { client_id, bytes } => {
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
                                }
                            } else {
                                // Default / legacy fallback
                                self.writer_unacked_bytes = (head.saturating_sub(offset)) as usize;
                            }
                        }
                        ActorCmd::Detach { client_id } => {
                            if let Some(cid) = client_id {
                                self.clients.remove(&cid);
                                if self.active_writer == Some(cid) {
                                    self.active_writer = None;
                                }
                            }
                            self.broadcast_presence();
                            if self.clients.is_empty() {
                                self.state = SessionState::Detached;
                                self.detached_at = Some(std::time::Instant::now());
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
                    }
                }
                // Invariant: Flow control checks writer_unacked_bytes only!
                // Slow viewers NEVER block the backend read or writer!
                Some(event) = backend_events.recv(), if self.writer_unacked_bytes < self.high_watermark => {
                    match event {
                        BackendEvent::Output(bytes) => {
                            // If ConPTY queries cursor position report (ESC [ 6 n),
                            // reply immediately so PowerShell does not block on startup!
                            if bytes.windows(4).any(|w| w == b"\x1b[6n") {
                                let _ = self.backend.write(b"\x1b[1;1R").await;
                            }

                            let start_offset = self.replay.head_offset();
                            self.replay.append(&bytes);
                            // Feed into VT parser / terminal
                            let mut parser = oxidite_vt::parser::Parser::new();
                            parser.advance(&mut self.terminal, &bytes);

                            self.writer_unacked_bytes += bytes.len();

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
    }
}
