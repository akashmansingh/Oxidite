use futures_util::{SinkExt, StreamExt};
use oxidite_proto::{
    AttachMode, BackendSpec, ClientMsg, ErrorCode, PROTOCOL_VERSION, ServerMsg, TermSize,
    pack_client_msg, unpack_server_msg,
};
use oxidite_server::auth::{UserRole, UserStore};
use oxidite_server::security::{RateLimiter, SecurityConfig};
use oxidite_server::{AppState, create_app};
use oxidite_session::manager::SessionManager;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

#[tokio::test]
async fn test_argon2_password_hashing() {
    let raw = "production-secure-pass!";
    let hash = UserStore::hash_password(raw).expect("hash ok");
    assert!(hash.starts_with("$argon2"));
    assert!(UserStore::verify_password(&hash, raw));
    assert!(!UserStore::verify_password(&hash, "wrong-pass"));
}

#[tokio::test]
async fn test_rate_limiter_window() {
    let limiter = RateLimiter::new();
    let client = "192.168.1.100";
    // Allow up to 3 requests in a 10s window
    assert!(limiter.check_limit(client, 3, Duration::from_secs(10)));
    assert!(limiter.check_limit(client, 3, Duration::from_secs(10)));
    assert!(limiter.check_limit(client, 3, Duration::from_secs(10)));
    // 4th request must be denied
    assert!(!limiter.check_limit(client, 3, Duration::from_secs(10)));
}

#[tokio::test]
async fn test_websocket_origin_rejection() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let state = AppState {
        manager: SessionManager::new(),
        token: "admin-token-123".into(),
        static_dir: "web/dist".into(),
        known_hosts: oxidite_ssh::KnownHostsStore::new(),
        security: SecurityConfig {
            allowed_origins: vec!["http://trusted.internal".into()],
            ..Default::default()
        },
        auth: UserStore::new(),
        shares: oxidite_server::share::ShareStore::new(),
        rate_limiter: RateLimiter::new(),
        node_id: "node-security-1".into(),
        directory: std::sync::Arc::new(oxidite_directory::InMemoryDirectory::new()),
    };

    let app = create_app(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let uri = format!("ws://{addr}/ws");
    let mut req = uri.into_client_request().unwrap();
    req.headers_mut().insert(
        "Origin",
        "https://malicious-attacker.evil.com".parse().unwrap(),
    );

    let connect_res = tokio_tungstenite::connect_async(req).await;
    // Disallowed origin must be rejected
    assert!(connect_res.is_err());
}

#[tokio::test]
async fn test_per_user_ownership_enforcement() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let user_store = UserStore::new();
    user_store
        .create_user("alice", "pass-alice", UserRole::User)
        .unwrap();
    user_store
        .create_user("bob", "pass-bob", UserRole::User)
        .unwrap();

    let token_alice = user_store
        .login("alice", "pass-alice", Duration::from_secs(3600))
        .unwrap()
        .token;
    let token_bob = user_store
        .login("bob", "pass-bob", Duration::from_secs(3600))
        .unwrap()
        .token;

    let state = AppState {
        manager: SessionManager::new(),
        token: "admin-bypass-token".into(),
        static_dir: "web/dist".into(),
        known_hosts: oxidite_ssh::KnownHostsStore::new(),
        security: SecurityConfig::default(),
        auth: user_store,
        shares: oxidite_server::share::ShareStore::new(),
        rate_limiter: RateLimiter::new(),
        node_id: "node-security-1".into(),
        directory: std::sync::Arc::new(oxidite_directory::InMemoryDirectory::new()),
    };

    let app = create_app(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let url = format!("ws://{addr}/ws");

    // 1. Alice connects and creates a session
    let (mut alice_ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let hello_alice = pack_client_msg(&ClientMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        token: token_alice,
    })
    .unwrap();
    alice_ws
        .send(Message::Binary(hello_alice.into()))
        .await
        .unwrap();
    let _welcome = alice_ws.next().await.unwrap().unwrap();

    let create = pack_client_msg(&ClientMsg::Create {
        backend: BackendSpec::LocalPty {
            shell: Some("cmd.exe".into()),
            cwd: None,
        },
        size: TermSize { rows: 24, cols: 80 },
    })
    .unwrap();
    alice_ws.send(Message::Binary(create.into())).await.unwrap();

    let msg = alice_ws.next().await.unwrap().unwrap();
    let server_msg = unpack_server_msg(&msg.into_data()).unwrap();
    let session_id = match server_msg {
        ServerMsg::Created { session } => session,
        other => panic!("Expected Created, got {:?}", other),
    };

    // 2. Bob connects with his own token
    let (mut bob_ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let hello_bob = pack_client_msg(&ClientMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        token: token_bob,
    })
    .unwrap();
    bob_ws
        .send(Message::Binary(hello_bob.into()))
        .await
        .unwrap();
    let _welcome = bob_ws.next().await.unwrap().unwrap();

    // 3. Bob attempts to attach to Alice's session -> MUST be rejected with Unauthorized
    let attach_bob = pack_client_msg(&ClientMsg::Attach {
        session: session_id,
        last_offset: None,
        mode: AttachMode::Viewer,
    })
    .unwrap();
    bob_ws
        .send(Message::Binary(attach_bob.into()))
        .await
        .unwrap();

    let msg = bob_ws.next().await.unwrap().unwrap();
    let server_msg = unpack_server_msg(&msg.into_data()).unwrap();
    match server_msg {
        ServerMsg::Error { code, message } => {
            assert_eq!(code, ErrorCode::Unauthorized);
            assert!(message.contains("not own"));
        }
        other => panic!("Expected Unauthorized Error, got: {:?}", other),
    }

    // Alice cleans up session
    let kill = pack_client_msg(&ClientMsg::Kill).unwrap();
    alice_ws.send(Message::Binary(kill.into())).await.unwrap();
}
