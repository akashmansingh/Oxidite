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
        host: "remote-gateway.lan".into(),
        port: 22,
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
    let msg = ws_stream.next().await.unwrap().unwrap();
    let server_msg = unpack_server_msg(&msg.into_data()).unwrap();
    assert!(
        matches!(server_msg, ServerMsg::HostKeyPrompt { ref host, .. } if host == "remote-gateway.lan")
    );

    // Accept host key
    let decision = pack_client_msg(&ClientMsg::HostKeyDecision { accept: true }).unwrap();
    ws_stream
        .send(Message::Binary(decision.into()))
        .await
        .unwrap();

    // Expect Created
    let msg = ws_stream.next().await.unwrap().unwrap();
    let server_msg = unpack_server_msg(&msg.into_data()).unwrap();
    assert!(matches!(server_msg, ServerMsg::Created { .. }));
}
