//! Local PTY backend implementation.

use super::{Backend, BackendEvent, Result, SessionError};
use async_trait::async_trait;
use oxidite_proto::TermSize;
use oxidite_pty::{PtySize, PtyWriter, spawn_in_directory};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

pub struct LocalPtyBackend {
    writer: Arc<Mutex<PtyWriter>>,
    reader_rx: Option<mpsc::Receiver<BackendEvent>>,
}

impl LocalPtyBackend {
    pub fn spawn(shell: Option<String>, cwd: Option<String>, size: TermSize) -> Result<Self> {
        let cmd = oxidite_pty::resolve_safe_shell(shell.as_deref())
            .map_err(|e| SessionError::Backend(e.to_string()))?;

        let pty_size = PtySize {
            rows: size.rows,
            cols: size.cols,
            px_w: 0,
            px_h: 0,
        };

        let (writer, mut reader) =
            spawn_in_directory(cmd, pty_size, cwd.as_deref().map(std::path::Path::new))
                .map_err(|e| SessionError::Backend(e.to_string()))?;

        let (tx, rx) = mpsc::channel(128);

        std::thread::Builder::new()
            .name("pty-reader-bridge".into())
            .spawn(move || {
                loop {
                    match reader.recv() {
                        Some(bytes) => {
                            if tx.blocking_send(BackendEvent::Output(bytes)).is_err() {
                                break;
                            }
                        }
                        None => {
                            let _ = tx.blocking_send(BackendEvent::Exited(None));
                            break;
                        }
                    }
                }
            })
            .map_err(SessionError::Io)?;

        Ok(Self {
            writer: Arc::new(Mutex::new(writer)),
            reader_rx: Some(rx),
        })
    }
}

#[async_trait]
impl Backend for LocalPtyBackend {
    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        let mut writer = self.writer.lock().await;
        writer
            .write(bytes)
            .map_err(|e| SessionError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn resize(&mut self, size: TermSize) -> Result<()> {
        let mut writer = self.writer.lock().await;
        writer
            .resize(PtySize {
                rows: size.rows,
                cols: size.cols,
                px_w: 0,
                px_h: 0,
            })
            .map_err(|e| SessionError::Backend(e.to_string()))
    }

    fn events(&mut self) -> mpsc::Receiver<BackendEvent> {
        self.reader_rx
            .take()
            .expect("events() called more than once")
    }

    async fn kill(&mut self) -> Result<()> {
        let mut writer = self.writer.lock().await;
        writer
            .kill()
            .map_err(|e| SessionError::Backend(e.to_string()))
    }
}
