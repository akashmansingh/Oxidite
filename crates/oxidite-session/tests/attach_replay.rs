use async_trait::async_trait;
use oxidite_proto::{AttachMode, ReplayStart, TermSize};
use oxidite_session::{
    Backend, BackendEvent, Result,
    actor::{ActorCmd, SessionActor},
};
use oxidite_vt::terminal::Terminal;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

struct MockBackend {
    rx: Option<mpsc::Receiver<BackendEvent>>,
}

#[async_trait]
impl Backend for MockBackend {
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
async fn test_attach_replay_within_buffer() {
    let sid = uuid::Uuid::new_v4();
    let (event_tx, event_rx) = mpsc::channel(128);
    let (cmd_tx, cmd_rx) = mpsc::channel(128);

    let backend = Box::new(MockBackend { rx: Some(event_rx) });
    let (actor, _) = SessionActor::new(sid, TermSize { rows: 24, cols: 80 }, backend, cmd_rx);

    tokio::spawn(actor.run());

    // Emit 100 bytes of output
    let chunk1 = b"Hello Oxidite Streaming World!\r\n";
    event_tx
        .send(BackendEvent::Output(chunk1.to_vec()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Attach asking from offset 6
    let (resp_tx, resp_rx) = oneshot::channel();
    cmd_tx
        .send(ActorCmd::Attach {
            mode: AttachMode::Writer,
            last_offset: Some(6),
            resp: resp_tx,
        })
        .await
        .unwrap();

    let attach_res = resp_rx.await.unwrap();
    assert_eq!(attach_res.start, ReplayStart::Replay { from: 6 });
    assert!(attach_res.snapshot.is_none());
    assert_eq!(attach_res.replay_chunks[0], b"Oxidite Streaming World!\r\n");
}

#[tokio::test]
async fn test_snapshot_fallback_and_convergence() {
    let mut term1 = Terminal::new(80, 24);
    let mut parser1 = oxidite_vt::parser::Parser::new();

    // Simulate terminal workload
    for i in 1..=50 {
        let line = format!("Counter log line {i}: \x1b[32mOK\x1b[0m\r\n");
        parser1.advance(&mut term1, line.as_bytes());
    }

    // Generate snapshot
    let snapshot_bytes = oxidite_vt::snapshot::serialize_snapshot(&term1);
    assert!(!snapshot_bytes.is_empty());

    // Parse snapshot into fresh terminal
    let mut term2 = Terminal::new(80, 24);
    let mut parser2 = oxidite_vt::parser::Parser::new();
    parser2.advance(&mut term2, &snapshot_bytes);

    // Verify grid lines match exactly
    for row in 0..24 {
        for col in 0..80 {
            assert_eq!(term1.grid[row][col].ch, term2.grid[row][col].ch);
            assert_eq!(term1.grid[row][col].fg, term2.grid[row][col].fg);
            assert_eq!(term1.grid[row][col].bg, term2.grid[row][col].bg);
        }
    }
}
