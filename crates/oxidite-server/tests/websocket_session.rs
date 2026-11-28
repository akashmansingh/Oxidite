#![allow(clippy::useless_conversion, clippy::field_reassign_with_default)]

use futures_util::{SinkExt, StreamExt};
use oxidite_proto::{
    AttachMode, BackendSpec, ClientMsg, PROTOCOL_VERSION, ServerMsg, TermSize, pack_client_msg,
    unpack_server_msg,
};
use oxidite_server::{AppState, create_app};
use oxidite_session::manager::SessionManager;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn test_websocket_session_lifecycle() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let token = "test-token-secret-12345".to_string();
    let state = AppState {
        manager: SessionManager::new(),
        token: token.clone(),
        static_dir: "web/dist".into(),
        known_hosts: oxidite_ssh::KnownHostsStore::new(),
        security: oxidite_server::security::SecurityConfig::default(),
        auth: oxidite_server::auth::UserStore::new(),
        shares: oxidite_server::share::ShareStore::new(),
        rate_limiter: oxidite_server::security::RateLimiter::new(),
        metrics: oxidite_server::metrics::MetricsCollector::new(),
        node_id: "node-test-1".into(),
        directory: std::sync::Arc::new(oxidite_directory::InMemoryDirectory::new()),
        devices: oxidite_server::devices::DeviceStore::default(),
        device_connections: oxidite_server::device_connections::DeviceConnectionRegistry::default(),
    };

    let app = create_app(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let url = format!("ws://{addr}/ws");
    let (mut ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    // 1. Send Hello handshake
    let hello = pack_client_msg(&ClientMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        token,
    })
    .unwrap();
    ws_stream.send(Message::Binary(hello.into())).await.unwrap();

    // 2. Receive Welcome
    let msg = tokio::time::timeout(Duration::from_secs(5), ws_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let server_msg = unpack_server_msg(&msg.into_data()).unwrap();
    assert!(matches!(server_msg, ServerMsg::Welcome { .. }));

    // 3. Create Session
    let create = pack_client_msg(&ClientMsg::Create {
        backend: BackendSpec::LocalPty {
            shell: None,
            cwd: None,
        },
        size: TermSize { rows: 24, cols: 80 },
    })
    .unwrap();
    ws_stream
        .send(Message::Binary(create.into()))
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), ws_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let server_msg = unpack_server_msg(&msg.into_data()).unwrap();
    let session_id = match server_msg {
        ServerMsg::Created { session } => session,
        other => panic!("Expected ServerMsg::Created, got: {:?}", other),
    };

    // 4. Attach to session
    let attach = pack_client_msg(&ClientMsg::Attach {
        session: session_id,
        last_offset: None,
        mode: AttachMode::Writer,
    })
    .unwrap();
    ws_stream
        .send(Message::Binary(attach.into()))
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), ws_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let server_msg = unpack_server_msg(&msg.into_data()).unwrap();
    assert!(matches!(server_msg, ServerMsg::Attached { .. }));

    // 5. Send Ping and receive Pong
    let ping = pack_client_msg(&ClientMsg::Ping { nonce: 99999 }).unwrap();
    ws_stream.send(Message::Binary(ping.into())).await.unwrap();

    // Loop until we get pong or output
    let mut got_pong = false;
    for _ in 0..10 {
        if let Ok(Some(Ok(m))) =
            tokio::time::timeout(Duration::from_secs(2), ws_stream.next()).await
        {
            let smsg = unpack_server_msg(&m.into_data()).unwrap();
            if let ServerMsg::Pong { nonce } = smsg {
                assert_eq!(nonce, 99999);
                got_pong = true;
                break;
            }
        }
    }
    assert!(got_pong);

    // 6. Send Kill
    let kill = pack_client_msg(&ClientMsg::Kill).unwrap();
    ws_stream.send(Message::Binary(kill.into())).await.unwrap();
}

#[tokio::test]
async fn test_websocket_ssh_host_key_prompt_and_acceptance() {
    let (ssh_addr, expected_fp) =
        oxidite_ssh::test_support::spawn_test_ssh_server("deploy", "hunter2").await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let token = "test-token-ssh-999".to_string();
    let state = AppState {
        manager: SessionManager::new(),
        token: token.clone(),
        static_dir: "web/dist".into(),
        known_hosts: oxidite_ssh::KnownHostsStore::new(),
        security: oxidite_server::security::SecurityConfig::default(),
        auth: oxidite_server::auth::UserStore::new(),
        shares: oxidite_server::share::ShareStore::new(),
        rate_limiter: oxidite_server::security::RateLimiter::new(),
        metrics: oxidite_server::metrics::MetricsCollector::new(),
        node_id: "node-test-1".into(),
        directory: std::sync::Arc::new(oxidite_directory::InMemoryDirectory::new()),
        devices: oxidite_server::devices::DeviceStore::default(),
        device_connections: oxidite_server::device_connections::DeviceConnectionRegistry::default(),
    };

    let app = create_app(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let url = format!("ws://{addr}/ws");
    let (mut ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    // Send Hello
    let hello = pack_client_msg(&ClientMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        token,
    })
    .unwrap();
    ws_stream.send(Message::Binary(hello.into())).await.unwrap();

    let _welcome = ws_stream.next().await.unwrap().unwrap();

    // Create SSH backend
    let ssh_spec = oxidite_proto::BackendSpec::Ssh {
        host: ssh_addr.ip().to_string(),
        port: ssh_addr.port(),
        user: "deploy".into(),
        auth: oxidite_proto::AuthSpec::Password("hunter2".into()),
        multiplexer: oxidite_proto::Multiplexer::Tmux,
    };
    let create = pack_client_msg(&ClientMsg::Create {
        backend: ssh_spec,
        size: TermSize { rows: 24, cols: 80 },
    })
    .unwrap();
    ws_stream
        .send(Message::Binary(create.into()))
        .await
        .unwrap();

    // Expect HostKeyPrompt
    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ws_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let server_msg = unpack_server_msg(&msg.into_data()).unwrap();
    let expected_host = ssh_addr.ip().to_string();
    assert!(
        matches!(
            server_msg,
            ServerMsg::HostKeyPrompt {
                ref host,
                ref fingerprint,
                known: oxidite_proto::HostKeyStatus::Unknown
            } if host == &expected_host && fingerprint == &expected_fp
        ),
        "unexpected server message: {server_msg:?}"
    );

    // Accept host key
    let decision = pack_client_msg(&ClientMsg::HostKeyDecision { accept: true }).unwrap();
    ws_stream
        .send(Message::Binary(decision.into()))
        .await
        .unwrap();

    // Expect Created
    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ws_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let server_msg = unpack_server_msg(&msg.into_data()).unwrap();
    let session_id = match server_msg {
        ServerMsg::Created { session } => session,
        other => panic!("expected Created, got {other:?}"),
    };

    // Attach and verify real SSH echo output
    let attach = pack_client_msg(&ClientMsg::Attach {
        session: session_id,
        last_offset: None,
        mode: oxidite_proto::AttachMode::Writer,
    })
    .unwrap();
    ws_stream
        .send(Message::Binary(attach.into()))
        .await
        .unwrap();

    let input = pack_client_msg(&ClientMsg::Input(b"hello-ssh\n".to_vec())).unwrap();
    ws_stream.send(Message::Binary(input.into())).await.unwrap();

    let mut collected = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(Message::Binary(data)))) =
            tokio::time::timeout(std::time::Duration::from_secs(2), ws_stream.next()).await
            && let Ok(ServerMsg::Output { bytes, .. }) = unpack_server_msg(&data)
        {
            collected.extend_from_slice(&bytes);
            if String::from_utf8_lossy(&collected).contains("hello-ssh") {
                break;
            }
        }
    }
    let output = String::from_utf8_lossy(&collected);
    assert!(
        output.contains("ready") && output.contains("hello-ssh"),
        "expected SSH banner and echo output, got {output:?}"
    );
}

#[tokio::test]
async fn test_websocket_ssh_mismatch_and_bad_password_refused() {
    let (ssh_addr, _fp) =
        oxidite_ssh::test_support::spawn_test_ssh_server("deploy", "hunter2").await;

    let known_hosts = oxidite_ssh::KnownHostsStore::new();
    known_hosts.trust(
        oxidite_ssh::host_key_id(&ssh_addr.ip().to_string(), ssh_addr.port()),
        "SHA256:stale-wrong-key".into(),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let token = "test-token-ssh-mismatch".to_string();
    let state = AppState {
        manager: SessionManager::new(),
        token: token.clone(),
        static_dir: "web/dist".into(),
        known_hosts,
        security: oxidite_server::security::SecurityConfig::default(),
        auth: oxidite_server::auth::UserStore::new(),
        shares: oxidite_server::share::ShareStore::new(),
        rate_limiter: oxidite_server::security::RateLimiter::new(),
        metrics: oxidite_server::metrics::MetricsCollector::new(),
        node_id: "node-test-1".into(),
        directory: std::sync::Arc::new(oxidite_directory::InMemoryDirectory::new()),
        devices: oxidite_server::devices::DeviceStore::default(),
        device_connections: oxidite_server::device_connections::DeviceConnectionRegistry::default(),
    };

    tokio::spawn(async move {
        axum::serve(listener, create_app(state)).await.unwrap();
    });

    let url = format!("ws://{addr}/ws");
    let (mut ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let hello = pack_client_msg(&ClientMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        token,
    })
    .unwrap();
    ws_stream.send(Message::Binary(hello.into())).await.unwrap();
    let _ = ws_stream.next().await.unwrap().unwrap();

    // 1. Mismatched host key is refused without prompting
    let create_mismatch = pack_client_msg(&ClientMsg::Create {
        backend: oxidite_proto::BackendSpec::Ssh {
            host: ssh_addr.ip().to_string(),
            port: ssh_addr.port(),
            user: "deploy".into(),
            auth: oxidite_proto::AuthSpec::Password("hunter2".into()),
            multiplexer: oxidite_proto::Multiplexer::None,
        },
        size: TermSize { rows: 24, cols: 80 },
    })
    .unwrap();
    ws_stream
        .send(Message::Binary(create_mismatch.into()))
        .await
        .unwrap();

    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ws_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let err_msg = unpack_server_msg(&msg.into_data()).unwrap();
    assert!(
        matches!(err_msg, ServerMsg::Error { ref message, .. } if message.to_lowercase().contains("mismatch")),
        "expected mismatch error, got {err_msg:?}"
    );

    // 2. Unknown host key accepted + wrong password must fail with Error
    let (ssh_addr2, _) =
        oxidite_ssh::test_support::spawn_test_ssh_server("deploy", "hunter2").await;
    let create_bad_pw = pack_client_msg(&ClientMsg::Create {
        backend: oxidite_proto::BackendSpec::Ssh {
            host: ssh_addr2.ip().to_string(),
            port: ssh_addr2.port(),
            user: "deploy".into(),
            auth: oxidite_proto::AuthSpec::Password("wrong-pw".into()),
            multiplexer: oxidite_proto::Multiplexer::None,
        },
        size: TermSize { rows: 24, cols: 80 },
    })
    .unwrap();
    ws_stream
        .send(Message::Binary(create_bad_pw.into()))
        .await
        .unwrap();

    let prompt = tokio::time::timeout(std::time::Duration::from_secs(5), ws_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        unpack_server_msg(&prompt.into_data()).unwrap(),
        ServerMsg::HostKeyPrompt { .. }
    ));

    let accept = pack_client_msg(&ClientMsg::HostKeyDecision { accept: true }).unwrap();
    ws_stream
        .send(Message::Binary(accept.into()))
        .await
        .unwrap();

    let err = tokio::time::timeout(std::time::Duration::from_secs(5), ws_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let err_msg = unpack_server_msg(&err.into_data()).unwrap();
    assert!(
        matches!(
            err_msg,
            ServerMsg::Error {
                code: oxidite_proto::ErrorCode::BackendError,
                ..
            }
        ),
        "expected BackendError for bad password, got {err_msg:?}"
    );
}

struct FastExitMockBackend {
    rx: Option<tokio::sync::mpsc::Receiver<oxidite_session::BackendEvent>>,
}

impl oxidite_session::Backend for FastExitMockBackend {
    fn write<'life0, 'life1, 'async_trait>(
        &'life0 mut self,
        _bytes: &'life1 [u8],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = oxidite_session::Result<()>> + Send + 'async_trait>,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
    fn resize<'life0, 'async_trait>(
        &'life0 mut self,
        _size: TermSize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = oxidite_session::Result<()>> + Send + 'async_trait>,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
    fn events(&mut self) -> tokio::sync::mpsc::Receiver<oxidite_session::BackendEvent> {
        self.rx.take().unwrap()
    }
    fn kill<'life0, 'async_trait>(
        &'life0 mut self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = oxidite_session::Result<()>> + Send + 'async_trait>,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn test_early_exit_attach_reports_exit_code_not_not_found() {
    let manager = SessionManager::new();
    let (ev_tx, ev_rx) = tokio::sync::mpsc::channel(4);
    ev_tx
        .send(oxidite_session::BackendEvent::Exited(Some(7)))
        .await
        .unwrap();
    let sid = uuid::Uuid::new_v4();
    manager.create_with_backend(
        sid,
        TermSize { rows: 24, cols: 80 },
        Box::new(FastExitMockBackend { rx: Some(ev_rx) }),
    );
    tokio::time::sleep(Duration::from_millis(30)).await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let token = "early-exit-token".to_string();
    let state = AppState {
        manager,
        token: token.clone(),
        static_dir: "web/dist".into(),
        known_hosts: oxidite_ssh::KnownHostsStore::new(),
        security: oxidite_server::security::SecurityConfig::default(),
        auth: oxidite_server::auth::UserStore::new(),
        shares: oxidite_server::share::ShareStore::new(),
        rate_limiter: oxidite_server::security::RateLimiter::new(),
        metrics: oxidite_server::metrics::MetricsCollector::new(),
        node_id: "node-early-exit".into(),
        directory: std::sync::Arc::new(oxidite_directory::InMemoryDirectory::new()),
        devices: oxidite_server::devices::DeviceStore::default(),
        device_connections: oxidite_server::device_connections::DeviceConnectionRegistry::default(),
    };
    tokio::spawn(async move {
        axum::serve(listener, create_app(state)).await.unwrap();
    });

    let url = format!("ws://{addr}/ws");
    let (mut ws_stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    ws_stream
        .send(Message::Binary(
            pack_client_msg(&ClientMsg::Hello {
                protocol_version: PROTOCOL_VERSION,
                token,
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    let _welcome = ws_stream.next().await.unwrap().unwrap();

    ws_stream
        .send(Message::Binary(
            pack_client_msg(&ClientMsg::Attach {
                session: sid,
                last_offset: None,
                mode: AttachMode::Writer,
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();

    let mut saw_exit_code = false;
    let mut saw_session_exited_err = false;
    for _ in 0..4 {
        if let Ok(Some(Ok(Message::Binary(data)))) =
            tokio::time::timeout(Duration::from_secs(2), ws_stream.next()).await
        {
            match unpack_server_msg(&data).unwrap() {
                ServerMsg::Exit { code: Some(7) } => saw_exit_code = true,
                ServerMsg::Error {
                    code: oxidite_proto::ErrorCode::SessionExited,
                    ..
                } => saw_session_exited_err = true,
                _ => {}
            }
        }
    }
    assert!(saw_exit_code, "expected ServerMsg::Exit with code 7");
    assert!(
        saw_session_exited_err,
        "expected ErrorCode::SessionExited instead of SessionNotFound"
    );
}
