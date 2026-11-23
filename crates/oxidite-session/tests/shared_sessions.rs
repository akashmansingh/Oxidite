use async_trait::async_trait;
use oxidite_proto::{AttachMode, ServerMsg, TermSize};
use oxidite_session::{
    Backend, BackendEvent, Result,
    actor::{ActorCmd, SessionActor},
};
use oxidite_vt::terminal::Terminal;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

struct SharedMockBackend {
    written: Arc<Mutex<Vec<u8>>>,
    event_rx: Option<mpsc::Receiver<BackendEvent>>,
}

#[async_trait]
impl Backend for SharedMockBackend {
    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.written.lock().unwrap().extend_from_slice(bytes);
        Ok(())
    }
    async fn resize(&mut self, _size: TermSize) -> Result<()> {
        Ok(())
    }
    fn events(&mut self) -> mpsc::Receiver<BackendEvent> {
        self.event_rx.take().unwrap()
    }
    async fn kill(&mut self) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn test_multiple_clients_converge_to_identical_screen() {
    let sid = uuid::Uuid::new_v4();
    let (event_tx, event_rx) = mpsc::channel(256);
    let (cmd_tx, cmd_rx) = mpsc::channel(256);

    let backend = Box::new(SharedMockBackend {
        written: Arc::new(Mutex::new(Vec::new())),
        event_rx: Some(event_rx),
    });
    let (actor, _) = SessionActor::new(sid, TermSize { rows: 24, cols: 80 }, backend, cmd_rx);
    tokio::spawn(actor.run());

    // 1. Attach Writer (client_id: 1)
    let (resp_tx1, resp_rx1) = oneshot::channel();
    cmd_tx
        .send(ActorCmd::Attach {
            mode: AttachMode::Writer,
            last_offset: None,
            client_id: Some(1),
            resp: resp_tx1,
        })
        .await
        .unwrap();
    let attach1 = resp_rx1.await.unwrap();
    let mut rx1 = attach1.rx;

    // 2. Attach Viewer A (client_id: 2)
    let (resp_tx2, resp_rx2) = oneshot::channel();
    cmd_tx
        .send(ActorCmd::Attach {
            mode: AttachMode::Viewer,
            last_offset: None,
            client_id: Some(2),
            resp: resp_tx2,
        })
        .await
        .unwrap();
    let attach2 = resp_rx2.await.unwrap();
    let mut rx2 = attach2.rx;

    // 3. Attach Viewer B (client_id: 3)
    let (resp_tx3, resp_rx3) = oneshot::channel();
    cmd_tx
        .send(ActorCmd::Attach {
            mode: AttachMode::Viewer,
            last_offset: None,
            client_id: Some(3),
            resp: resp_tx3,
        })
        .await
        .unwrap();
    let attach3 = resp_rx3.await.unwrap();
    let mut rx3 = attach3.rx;

    // Produce terminal data with colors and cursor movement
    let sample =
        b"Line 1: Hello from session actor!\r\n\x1b[32mLine 2: Green colored text\x1b[0m\r\n";
    event_tx
        .send(BackendEvent::Output(sample.to_vec()))
        .await
        .unwrap();

    let mut term_writer = Terminal::new(80, 24);
    let mut term_viewer_a = Terminal::new(80, 24);
    let mut term_viewer_b = Terminal::new(80, 24);

    let mut p1 = oxidite_vt::parser::Parser::new();
    let mut p2 = oxidite_vt::parser::Parser::new();
    let mut p3 = oxidite_vt::parser::Parser::new();

    // Collect outputs across all 3 clients
    for _ in 0..5 {
        if let Ok(Ok(ServerMsg::Output { bytes, .. })) =
            tokio::time::timeout(Duration::from_millis(50), rx1.recv()).await
        {
            p1.advance(&mut term_writer, &bytes);
        }
        if let Ok(Ok(ServerMsg::Output { bytes, .. })) =
            tokio::time::timeout(Duration::from_millis(50), rx2.recv()).await
        {
            p2.advance(&mut term_viewer_a, &bytes);
        }
        if let Ok(Ok(ServerMsg::Output { bytes, .. })) =
            tokio::time::timeout(Duration::from_millis(50), rx3.recv()).await
        {
            p3.advance(&mut term_viewer_b, &bytes);
        }
    }

    // Verify all 3 screens converge identically
    assert_eq!(term_writer.grid[0][0].ch, term_viewer_a.grid[0][0].ch);
    assert_eq!(term_viewer_a.grid[0][0].ch, term_viewer_b.grid[0][0].ch);
    assert_eq!(term_writer.grid[1][7].fg, term_viewer_a.grid[1][7].fg);
}

#[tokio::test]
async fn test_stalled_viewer_does_not_delay_writer() {
    let sid = uuid::Uuid::new_v4();
    let (event_tx, event_rx) = mpsc::channel(1024);
    let (cmd_tx, cmd_rx) = mpsc::channel(1024);

    let backend = Box::new(SharedMockBackend {
        written: Arc::new(Mutex::new(Vec::new())),
        event_rx: Some(event_rx),
    });
    let (actor, _) = SessionActor::new(sid, TermSize { rows: 24, cols: 80 }, backend, cmd_rx);
    tokio::spawn(actor.run());

    // 1. Attach Writer (client 1) and Viewer (client 2)
    let (resp_tx1, resp_rx1) = oneshot::channel();
    cmd_tx
        .send(ActorCmd::Attach {
            mode: AttachMode::Writer,
            last_offset: None,
            client_id: Some(1),
            resp: resp_tx1,
        })
        .await
        .unwrap();
    let _attach1 = resp_rx1.await.unwrap();

    let (resp_tx2, resp_rx2) = oneshot::channel();
    cmd_tx
        .send(ActorCmd::Attach {
            mode: AttachMode::Viewer,
            last_offset: None,
            client_id: Some(2),
            resp: resp_tx2,
        })
        .await
        .unwrap();
    let _attach2 = resp_rx2.await.unwrap();

    // 2. Stream 2 MiB of data in chunks. The writer sends Acks, but the viewer NEVER sends Acks!
    let chunk = vec![b'A'; 64 * 1024]; // 64 KiB
    let mut total_sent = 0;

    for _ in 0..32 {
        event_tx
            .send(BackendEvent::Output(chunk.clone()))
            .await
            .unwrap();
        total_sent += chunk.len() as u64;

        // Writer keeps acknowledging
        cmd_tx
            .send(ActorCmd::Ack {
                client_id: Some(1),
                offset: total_sent,
            })
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    // 3. Verify writer was NEVER stalled: backend continues accepting events!
    let sent_ok = event_tx
        .send(BackendEvent::Output(b"Final Chunk".to_vec()))
        .await
        .is_ok();
    assert!(sent_ok);
}

#[tokio::test]
async fn test_single_writer_enforcement_and_takeover() {
    let sid = uuid::Uuid::new_v4();
    let (_event_tx, event_rx) = mpsc::channel(64);
    let (cmd_tx, cmd_rx) = mpsc::channel(64);

    let written = Arc::new(Mutex::new(Vec::new()));
    let backend = Box::new(SharedMockBackend {
        written: written.clone(),
        event_rx: Some(event_rx),
    });
    let (actor, _) = SessionActor::new(sid, TermSize { rows: 24, cols: 80 }, backend, cmd_rx);
    tokio::spawn(actor.run());

    // Client 1 attaches as Writer
    let (resp_tx1, resp_rx1) = oneshot::channel();
    cmd_tx
        .send(ActorCmd::Attach {
            mode: AttachMode::Writer,
            last_offset: None,
            client_id: Some(1),
            resp: resp_tx1,
        })
        .await
        .unwrap();
    let _ = resp_rx1.await.unwrap();

    // Client 2 attaches as Viewer
    let (resp_tx2, resp_rx2) = oneshot::channel();
    cmd_tx
        .send(ActorCmd::Attach {
            mode: AttachMode::Viewer,
            last_offset: None,
            client_id: Some(2),
            resp: resp_tx2,
        })
        .await
        .unwrap();
    let _ = resp_rx2.await.unwrap();

    // Client 2 (Viewer) attempts input -> MUST be dropped / ignored
    cmd_tx
        .send(ActorCmd::Input {
            client_id: Some(2),
            bytes: b"Viewer unauthorized input\n".to_vec(),
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(written.lock().unwrap().is_empty());

    // Client 1 (Writer) sends input -> Accepted
    cmd_tx
        .send(ActorCmd::Input {
            client_id: Some(1),
            bytes: b"Writer valid input\n".to_vec(),
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(&*written.lock().unwrap(), b"Writer valid input\n");

    // Client 2 takes over writer role
    let (to_tx, to_rx) = oneshot::channel();
    cmd_tx
        .send(ActorCmd::TakeoverWriter {
            client_id: 2,
            resp: to_tx,
        })
        .await
        .unwrap();
    assert!(to_rx.await.unwrap());

    // Now Client 2 is the writer! Client 2's input is accepted
    cmd_tx
        .send(ActorCmd::Input {
            client_id: Some(2),
            bytes: b"New writer input\n".to_vec(),
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        &*written.lock().unwrap(),
        b"Writer valid input\nNew writer input\n"
    );
}
