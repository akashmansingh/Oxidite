//! Session actor owning terminal state, replay buffer, and backend.

use super::{Backend, BackendEvent, replay::ReplayBuffer};
use oxidite_proto::{
    AttachMode, Offset, ReplayStart, ServerMsg, SessionId, SessionState, TermSize,
};
use oxidite_vt::terminal::Terminal;
use tokio::sync::{broadcast, mpsc, oneshot};

pub const HIGH_WATERMARK: usize = 1024 * 1024; // 1 MiB
pub const LOW_WATERMARK: usize = 256 * 1024; // 256 KiB
pub const DEFAULT_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3600);

pub struct SessionActor {
    pub id: SessionId,
    pub terminal: Terminal,
    pub replay: ReplayBuffer,
    pub state: SessionState,
    pub backend: Box<dyn Backend>,
    pub receiver: mpsc::Receiver<ActorCmd>,
    pub client_tx: broadcast::Sender<ServerMsg>,
    pub active_writer: Option<u64>,
    pub unacked_bytes: usize,
    pub high_watermark: usize,
    pub low_watermark: usize,
    pub detached_at: Option<std::time::Instant>,
    pub idle_timeout: std::time::Duration,
}

pub enum ActorCmd {
    Attach {
        mode: AttachMode,
        last_offset: Option<Offset>,
        resp: oneshot::Sender<AttachResult>,
    },
    Input {
        bytes: Vec<u8>,
    },
    Resize {
        size: TermSize,
    },
    Ack {
        offset: Offset,
    },
    Detach,
    Kill,
    GetState {
        resp: oneshot::Sender<SessionState>,
    },
}

pub struct AttachResult {
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
        let (client_tx, _) = broadcast::channel(256);
        let actor = Self {
            id,
            terminal: Terminal::new(size.cols.into(), size.rows.into()),
            replay: ReplayBuffer::new(8 * 1024 * 1024), // 8 MiB default
            state: SessionState::Starting,
            backend,
            receiver,
            client_tx: client_tx.clone(),
            active_writer: None,
            unacked_bytes: 0,
            high_watermark: HIGH_WATERMARK,
            low_watermark: LOW_WATERMARK,
            detached_at: None,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
        };
        (actor, client_tx)
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
                        ActorCmd::Attach { mode: _, last_offset, resp } => {
                            self.detached_at = None;
                            let head = self.replay.head_offset();
                            let tail = self.replay.tail_offset();
                            let attach_res = if let Some(last) = last_offset {
                                if last >= tail && last <= head {
                                    let chunk = self.replay.get_from(last).unwrap_or_default();
                                    AttachResult {
                                        start: ReplayStart::Replay { from: last },
                                        snapshot: None,
                                        replay_chunks: vec![chunk],
                                        current_offset: head,
                                        rx: self.client_tx.subscribe(),
                                    }
                                } else {
                                    let snapshot_bytes = oxidite_vt::snapshot::serialize_snapshot(&self.terminal);
                                    AttachResult {
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
                                    start: ReplayStart::Snapshot { at: head },
                                    snapshot: Some(snapshot_bytes),
                                    replay_chunks: Vec::new(),
                                    current_offset: head,
                                    rx: self.client_tx.subscribe(),
                                }
                            };
                            let _ = resp.send(attach_res);
                        }
                        ActorCmd::Input { bytes } => {
                            let _ = self.backend.write(&bytes).await;
                        }
                        ActorCmd::Resize { size } => {
                            self.terminal.resize(size.cols.into(), size.rows.into());
                            let _ = self.backend.resize(size).await;
                        }
                        ActorCmd::Ack { offset } => {
                            let head = self.replay.head_offset();
                            self.unacked_bytes = (head.saturating_sub(offset)) as usize;
                        }
                        ActorCmd::Detach => {
                            if self.client_tx.receiver_count() <= 1 {
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
                    }
                }
                Some(event) = backend_events.recv(), if self.unacked_bytes < self.high_watermark => {
                    match event {
                        BackendEvent::Output(bytes) => {
                            let start_offset = self.replay.head_offset();
                            self.replay.append(&bytes);
                            // Feed into VT parser / terminal
                            let mut parser = oxidite_vt::parser::Parser::new();
                            parser.advance(&mut self.terminal, &bytes);

                            self.unacked_bytes += bytes.len();
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
