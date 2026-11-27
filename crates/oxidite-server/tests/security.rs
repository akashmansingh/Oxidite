#![allow(clippy::useless_conversion, clippy::field_reassign_with_default)]

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
        metrics: oxidite_server::metrics::MetricsCollector::new(),
        node_id: "node-security-1".into(),
        directory: std::sync::Arc::new(oxidite_directory::InMemoryDirectory::new()),
        devices: oxidite_server::devices::DeviceStore::default(),
        device_connections: oxidite_server::device_connections::DeviceConnectionRegistry::default(),
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
        metrics: oxidite_server::metrics::MetricsCollector::new(),
        node_id: "node-security-1".into(),
        directory: std::sync::Arc::new(oxidite_directory::InMemoryDirectory::new()),
        devices: oxidite_server::devices::DeviceStore::default(),
        device_connections: oxidite_server::device_connections::DeviceConnectionRegistry::default(),
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
            shell: None,
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

#[tokio::test]
async fn test_same_origin_websocket_allowed_and_cross_origin_rejected() {
    let sec = SecurityConfig::default();
    assert!(sec.is_origin_allowed_for_host(Some("http://127.0.0.1:3030"), Some("127.0.0.1:3030")));
    assert!(sec.is_origin_allowed_for_host(Some("https://localhost:8443"), Some("localhost:8443")));
    assert!(sec.is_origin_allowed_for_host(
        Some("https://gw.example.net:8443"),
        Some("gw.example.net:8443")
    ));
    assert!(!sec.is_origin_allowed_for_host(
        Some("http://localhost.evil.com:3030"),
        Some("gw.example.net:8443")
    ));
    assert!(!sec.is_origin_allowed_for_host(Some("https://evil.com"), Some("gw.example.net:8443")));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let token = "origin-test-token".to_string();
    let state = AppState {
        manager: SessionManager::new(),
        token,
        static_dir: "web/dist".into(),
        known_hosts: oxidite_ssh::KnownHostsStore::new(),
        security: SecurityConfig::default(),
        auth: oxidite_server::auth::UserStore::new(),
        shares: oxidite_server::share::ShareStore::new(),
        rate_limiter: RateLimiter::new(),
        metrics: oxidite_server::metrics::MetricsCollector::new(),
        node_id: "node-security-origin".into(),
        directory: std::sync::Arc::new(oxidite_directory::InMemoryDirectory::new()),
        devices: oxidite_server::devices::DeviceStore::default(),
        device_connections: oxidite_server::device_connections::DeviceConnectionRegistry::default(),
    };
    tokio::spawn(async move {
        axum::serve(listener, create_app(state)).await.unwrap();
    });

    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let url = format!("ws://{addr}/ws");

    // 1. Remote LAN/domain same-origin WebSocket upgrade succeeds
    let mut same_origin_req = url.as_str().into_client_request().unwrap();
    same_origin_req
        .headers_mut()
        .insert("host", "gateway.local:8443".parse().unwrap());
    same_origin_req
        .headers_mut()
        .insert("origin", "https://gateway.local:8443".parse().unwrap());
    let (mut ws_ok, _) = tokio_tungstenite::connect_async(same_origin_req)
        .await
        .unwrap();
    let _ = ws_ok.close(None).await;

    // 2. Cross-origin WebSocket upgrade is rejected with 403 Forbidden
    let mut evil_req = url.as_str().into_client_request().unwrap();
    evil_req
        .headers_mut()
        .insert("host", "gateway.local:8443".parse().unwrap());
    evil_req
        .headers_mut()
        .insert("origin", "https://attacker.example".parse().unwrap());
    let err = tokio_tungstenite::connect_async(evil_req)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("403"), "expected 403, got {err}");
}

#[tokio::test]
async fn test_cors_and_security_headers_and_ssh_key_jail() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut sec = SecurityConfig::default();
    sec.bind_addr = Some("0.0.0.0:3030".parse().unwrap());
    sec.allow_non_loopback_pty = false;
    sec.ssh_key_dir = None;

    // Server-side SSH private key files must be blocked on non-loopback binds without ssh_key_dir
    assert!(sec.validate_ssh_key_path("~/.ssh/id_ed25519").is_err());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let token = "cors-headers-token".to_string();
    let state = AppState {
        manager: SessionManager::new(),
        token: token.clone(),
        static_dir: "web/dist".into(),
        known_hosts: oxidite_ssh::KnownHostsStore::new(),
        security: sec,
        auth: oxidite_server::auth::UserStore::new(),
        shares: oxidite_server::share::ShareStore::new(),
        rate_limiter: RateLimiter::new(),
        metrics: oxidite_server::metrics::MetricsCollector::new(),
        node_id: "node-security-cors".into(),
        directory: std::sync::Arc::new(oxidite_directory::InMemoryDirectory::new()),
        devices: oxidite_server::devices::DeviceStore::default(),
        device_connections: oxidite_server::device_connections::DeviceConnectionRegistry::default(),
    };
    tokio::spawn(async move {
        axum::serve(listener, create_app(state)).await.unwrap();
    });

    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "GET /api/sessions HTTP/1.1\r\nHost: gateway.example.com\r\nOrigin: https://evil.example\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.unwrap();
    let lower = raw.to_ascii_lowercase();
    assert!(
        !lower.contains("access-control-allow-origin:"),
        "untrusted origin must not receive CORS header: {raw}"
    );
    assert!(lower.contains("cross-origin-resource-policy: same-origin"));
    assert!(lower.contains("x-frame-options: deny"));
    assert!(lower.contains("x-content-type-options: nosniff"));
    assert!(lower.contains("strict-transport-security:"));
}

#[tokio::test]
async fn test_tailscale_origin_and_forwarded_host_security() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut sec = SecurityConfig::default();
    sec.allowed_origins = vec!["https://victus.tail999.ts.net".into()];

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let token = "ts-origin-token-123".to_string();
    let state = AppState {
        manager: SessionManager::new(),
        token: token.clone(),
        static_dir: "web/dist".into(),
        known_hosts: oxidite_ssh::KnownHostsStore::new(),
        security: sec,
        auth: UserStore::new(),
        shares: oxidite_server::share::ShareStore::new(),
        rate_limiter: RateLimiter::new(),
        metrics: oxidite_server::metrics::MetricsCollector::new(),
        node_id: "node-security-ts".into(),
        directory: std::sync::Arc::new(oxidite_directory::InMemoryDirectory::new()),
        devices: oxidite_server::devices::DeviceStore::default(),
        device_connections: oxidite_server::device_connections::DeviceConnectionRegistry::default(),
    };
    tokio::spawn(async move {
        axum::serve(
            listener,
            create_app(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });

    let ws_url = format!("ws://{addr}/ws");

    // 1. Exact configured Tailscale Origin is allowed over loopback forwarder.
    let mut allowed_req = ws_url.as_str().into_client_request().unwrap();
    allowed_req
        .headers_mut()
        .insert("host", addr.to_string().parse().unwrap());
    allowed_req
        .headers_mut()
        .insert("x-forwarded-host", "victus.tail999.ts.net".parse().unwrap());
    allowed_req
        .headers_mut()
        .insert("origin", "https://victus.tail999.ts.net".parse().unwrap());
    let (mut ws_ok, _) = tokio_tungstenite::connect_async(allowed_req).await.unwrap();
    let _ = ws_ok.close(None).await;

    // 2. Arbitrary *.ts.net Origin must be rejected even if X-Forwarded-Host claims to match it.
    let mut spoof_req = ws_url.as_str().into_client_request().unwrap();
    spoof_req
        .headers_mut()
        .insert("host", addr.to_string().parse().unwrap());
    spoof_req.headers_mut().insert(
        "x-forwarded-host",
        "attacker.tail999.ts.net".parse().unwrap(),
    );
    spoof_req
        .headers_mut()
        .insert("origin", "https://attacker.tail999.ts.net".parse().unwrap());
    let err = tokio_tungstenite::connect_async(spoof_req)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("403"),
        "expected 403 for arbitrary *.ts.net origin, got {err}"
    );

    // 3. Pairing grant honors trusted X-Forwarded-Host from loopback when in allowlist.
    let grant_body = serde_json::json!({ "ttl_secs": 300 }).to_string();
    let mut ok_grant = tokio::net::TcpStream::connect(addr).await.unwrap();
    let ok_req = format!(
        "POST /api/devices/grant HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\nX-Forwarded-Host: victus.tail999.ts.net\r\nX-Forwarded-Proto: https\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        grant_body.len(),
        grant_body
    );
    ok_grant.write_all(ok_req.as_bytes()).await.unwrap();
    let mut ok_raw = String::new();
    ok_grant.read_to_string(&mut ok_raw).await.unwrap();
    let (_, ok_json) = ok_raw.split_once("\r\n\r\n").unwrap();
    let ok_val: serde_json::Value = serde_json::from_str(ok_json).unwrap();
    assert!(
        ok_val["share_url"]
            .as_str()
            .is_some_and(|u| u.starts_with("https://victus.tail999.ts.net/#pair=")),
        "expected share_url with trusted tailscale host, got {ok_val}"
    );

    // 4. Spoofed X-Forwarded-Host outside allowlist must be ignored in share_url.
    let mut bad_grant = tokio::net::TcpStream::connect(addr).await.unwrap();
    let bad_req = format!(
        "POST /api/devices/grant HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\nX-Forwarded-Host: attacker.tail999.ts.net\r\nX-Forwarded-Proto: https\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        grant_body.len(),
        grant_body
    );
    bad_grant.write_all(bad_req.as_bytes()).await.unwrap();
    let mut bad_raw = String::new();
    bad_grant.read_to_string(&mut bad_raw).await.unwrap();
    let (_, bad_json) = bad_raw.split_once("\r\n\r\n").unwrap();
    let bad_val: serde_json::Value = serde_json::from_str(bad_json).unwrap();
    assert!(
        bad_val["share_url"]
            .as_str()
            .is_some_and(|u| !u.contains("attacker.tail999.ts.net")),
        "spoofed forwarded host must not leak into share_url: {bad_val}"
    );
}
