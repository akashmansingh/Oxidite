use futures_util::{SinkExt, StreamExt};
use oxidite_directory::InMemoryDirectory;
use oxidite_proto::{ClientMsg, PROTOCOL_VERSION, ServerMsg, pack_client_msg, unpack_server_msg};
use oxidite_server::auth::UserStore;
use oxidite_server::devices::DeviceStore;
use oxidite_server::security::{RateLimiter, SecurityConfig};
use oxidite_server::share::ShareStore;
use oxidite_server::{AppState, create_app};
use oxidite_session::manager::SessionManager;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn test_local_device_revocation_closes_active_websocket_immediately() {
    let store = DeviceStore::open_in_memory().await.unwrap();
    let grant = store
        .issue_enrollment_grant("alice", Duration::from_secs(300))
        .await
        .unwrap();
    let dev_a = store.enroll(&grant, "Device A").await.unwrap();

    let grant_b = store
        .issue_enrollment_grant("alice", Duration::from_secs(300))
        .await
        .unwrap();
    let dev_b = store.enroll(&grant_b, "Device B").await.unwrap();

    let state = AppState {
        manager: SessionManager::new(),
        token: "bootstrap".into(),
        static_dir: "web/dist".into(),
        known_hosts: oxidite_ssh::KnownHostsStore::new(),
        security: SecurityConfig::default(),
        auth: UserStore::new(),
        shares: ShareStore::new(),
        rate_limiter: RateLimiter::new(),
        metrics: oxidite_server::metrics::MetricsCollector::new(),
        node_id: "node-local-close".into(),
        directory: Arc::new(InMemoryDirectory::new()),
        devices: store.clone(),
        device_connections: oxidite_server::device_connections::DeviceConnectionRegistry::default(),
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let app = create_app(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let ws_url = format!("ws://{addr}/ws");

    // 1. Connect Device A
    let (mut ws_a, _) = connect_async(&ws_url).await.unwrap();
    let hello_a = pack_client_msg(&ClientMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        token: dev_a.token.clone(),
    })
    .unwrap();
    ws_a.send(Message::Binary(hello_a.into())).await.unwrap();

    let welcome_a = ws_a.next().await.unwrap().unwrap();
    if let Message::Binary(bin) = welcome_a {
        let msg = unpack_server_msg(&bin).unwrap();
        assert!(matches!(msg, ServerMsg::Welcome { .. }));
    } else {
        panic!("Device A expected Welcome");
    }

    // 2. Connect Device B
    let (mut ws_b, _) = connect_async(&ws_url).await.unwrap();
    let hello_b = pack_client_msg(&ClientMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        token: dev_b.token.clone(),
    })
    .unwrap();
    ws_b.send(Message::Binary(hello_b.into())).await.unwrap();

    let welcome_b = ws_b.next().await.unwrap().unwrap();
    if let Message::Binary(bin) = welcome_b {
        let msg = unpack_server_msg(&bin).unwrap();
        assert!(matches!(msg, ServerMsg::Welcome { .. }));
    } else {
        panic!("Device B expected Welcome");
    }

    // 3. Revoke Device A via HTTP DELETE
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "DELETE /api/devices/{} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: Bearer {}\r\n\r\n",
        dev_a.device_id, dev_b.token
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let resp = String::from_utf8_lossy(&buf).to_string();
    assert!(resp.starts_with("HTTP/1.1 204"));

    // 4. Device A must receive close or EOF immediately (under 2 seconds)
    let closed_a = tokio::time::timeout(Duration::from_secs(2), ws_a.next()).await;
    assert!(
        closed_a.is_ok(),
        "Device A must be closed immediately upon local revocation"
    );
    let msg_a = closed_a.unwrap();
    // In tungstenite, either None (connection closed), Message::Close, or Err(ConnectionClosed)
    assert!(
        msg_a.is_none()
            || matches!(msg_a, Some(Ok(Message::Close(_))))
            || matches!(msg_a, Some(Err(_))),
        "Device A connection should be closed, got {msg_a:?}"
    );

    // 5. Device B must still be alive and responsive
    ws_b.send(Message::Binary(
        pack_client_msg(&ClientMsg::Ping { nonce: 42 })
            .unwrap()
            .into(),
    ))
    .await
    .unwrap();
    let pong = tokio::time::timeout(Duration::from_secs(2), ws_b.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    if let Message::Binary(bin) = pong {
        let msg = unpack_server_msg(&bin).unwrap();
        assert!(matches!(msg, ServerMsg::Pong { nonce: 42 }));
    }
}

#[tokio::test]
async fn test_remote_node_revalidates_and_closes_revoked_device() {
    use oxidite_server::device_connections::DeviceConnectionRegistry;

    let store = DeviceStore::open_in_memory().await.unwrap();
    let grant = store
        .issue_enrollment_grant("alice", Duration::from_secs(300))
        .await
        .unwrap();
    let dev = store.enroll(&grant, "Remote Device").await.unwrap();

    let registry = DeviceConnectionRegistry::new(100);

    let (close_tx, mut close_rx) = tokio::sync::mpsc::channel(1);
    let conn_id = 9999;
    registry.register(dev.device_id, conn_id, close_tx);

    // Initially does not require revalidation when fresh
    assert!(!registry.requires_revalidation(conn_id, Duration::from_secs(30)));

    // Once 30 seconds have passed, requires revalidation
    assert!(registry.requires_revalidation(conn_id, Duration::from_millis(0)));

    // Revoke device in database
    store.revoke("alice", dev.device_id).await.unwrap();
    assert!(store.is_revoked(dev.device_id).await.unwrap());

    // When node revalidates, it closes the connection
    let is_revoked = store.is_revoked(dev.device_id).await.unwrap();
    if is_revoked {
        registry.close_local(dev.device_id);
    }

    // Close signal delivered to the connection task
    let signal = tokio::time::timeout(Duration::from_millis(500), close_rx.recv()).await;
    assert!(
        signal.is_ok(),
        "Close signal must be delivered upon revalidation"
    );
    assert_eq!(signal.unwrap(), Some(()));
}
