use oxidite_server::devices::{DeviceAuthError, DeviceStore};
use std::time::Duration;
use uuid::Uuid;

#[tokio::test]
async fn test_device_enrollment_and_authentication() {
    let store = DeviceStore::open_in_memory().await.unwrap();

    // 1. Issue an enrollment grant for user "alice"
    let grant = store
        .issue_enrollment_grant("alice", Duration::from_secs(300))
        .await
        .unwrap();

    // 2. Enroll device using the grant
    let enrolled = store.enroll(&grant, "Alice Laptop").await.unwrap();
    assert_eq!(enrolled.user_id, "alice");
    assert_eq!(enrolled.label, "Alice Laptop");
    assert!(enrolled.token.starts_with("od1."));

    let parts: Vec<&str> = enrolled.token.split('.').collect();
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0], "od1");
    let parsed_id = Uuid::parse_str(parts[1]).unwrap();
    assert_eq!(parsed_id, enrolled.device_id);
    assert!(parts[2].len() >= 32); // high entropy secret

    // 3. Authenticate with the token
    let principal = store.authenticate(&enrolled.token).await.unwrap();
    assert_eq!(principal.user_id, "alice");
    assert_eq!(principal.device_id, enrolled.device_id);

    // 4. Grant is single-use: enrolling again with same grant must fail
    let err = store.enroll(&grant, "Alice Phone").await.unwrap_err();
    assert!(matches!(err, DeviceAuthError::InvalidGrant));
}

#[tokio::test]
async fn test_generic_malformed_token_rejection() {
    let store = DeviceStore::open_in_memory().await.unwrap();

    let malformed_tokens = [
        "",
        "token",
        "od1",
        "od1.",
        "od1.not-a-uuid.secret",
        "od1.00000000-0000-0000-0000-000000000000",
        "od1.00000000-0000-0000-0000-000000000000.short",
        "bearer-static-token",
        "your-custom-token",
        "oxidite-secret",
    ];

    for bad in malformed_tokens {
        let res = store.authenticate(bad).await;
        assert!(
            matches!(res, Err(DeviceAuthError::Unauthorized)),
            "Token {bad:?} should fail with Unauthorized, got {res:?}"
        );
    }
}

#[tokio::test]
async fn test_owner_isolation_and_device_listing() {
    let store = DeviceStore::open_in_memory().await.unwrap();

    // Alice enrolls two devices
    let g1 = store
        .issue_enrollment_grant("alice", Duration::from_secs(300))
        .await
        .unwrap();
    let d_a1 = store.enroll(&g1, "Laptop").await.unwrap();

    let g2 = store
        .issue_enrollment_grant("alice", Duration::from_secs(300))
        .await
        .unwrap();
    let d_a2 = store.enroll(&g2, "Phone").await.unwrap();

    // Bob enrolls one device
    let g3 = store
        .issue_enrollment_grant("bob", Duration::from_secs(300))
        .await
        .unwrap();
    let d_b1 = store.enroll(&g3, "Workstation").await.unwrap();

    // Alice lists only her devices
    let alice_devices = store.list_for_user("alice").await.unwrap();
    assert_eq!(alice_devices.len(), 2);
    let alice_ids: Vec<Uuid> = alice_devices.iter().map(|d| d.device_id).collect();
    assert!(alice_ids.contains(&d_a1.device_id));
    assert!(alice_ids.contains(&d_a2.device_id));
    assert!(!alice_ids.contains(&d_b1.device_id));

    // Bob lists only his devices
    let bob_devices = store.list_for_user("bob").await.unwrap();
    assert_eq!(bob_devices.len(), 1);
    assert_eq!(bob_devices[0].device_id, d_b1.device_id);

    // Alice attempting to revoke Bob's device must fail and not revoke Bob's device
    let revoke_err = store.revoke("alice", d_b1.device_id).await.unwrap_err();
    assert!(matches!(
        revoke_err,
        DeviceAuthError::Unauthorized | DeviceAuthError::NotFound
    ));

    // Bob's device is still valid
    let principal = store.authenticate(&d_b1.token).await.unwrap();
    assert_eq!(principal.user_id, "bob");
}

#[tokio::test]
async fn test_device_revocation_invalidates_auth() {
    let store = DeviceStore::open_in_memory().await.unwrap();

    let g1 = store
        .issue_enrollment_grant("alice", Duration::from_secs(300))
        .await
        .unwrap();
    let dev1 = store.enroll(&g1, "Device 1").await.unwrap();

    let g2 = store
        .issue_enrollment_grant("alice", Duration::from_secs(300))
        .await
        .unwrap();
    let dev2 = store.enroll(&g2, "Device 2").await.unwrap();

    // Revoke device 1
    store.revoke("alice", dev1.device_id).await.unwrap();

    // Device 1 fails authentication
    let auth1 = store.authenticate(&dev1.token).await;
    assert!(matches!(auth1, Err(DeviceAuthError::Unauthorized)));

    // Device 2 still succeeds
    let auth2 = store.authenticate(&dev2.token).await.unwrap();
    assert_eq!(auth2.device_id, dev2.device_id);
}

#[tokio::test]
async fn test_raw_secrets_never_stored_in_database() {
    let store = DeviceStore::open_in_memory().await.unwrap();

    let grant = store
        .issue_enrollment_grant("alice", Duration::from_secs(300))
        .await
        .unwrap();
    let dev = store.enroll(&grant, "SecretCheck").await.unwrap();

    let raw_secret = dev.token.split('.').nth(2).unwrap();

    // Directly query database to confirm secret is stored as Argon2id hash, not plaintext
    let hashes = store.debug_get_all_hashes_for_test().await.unwrap();
    assert_eq!(hashes.len(), 1);
    let stored_hash = &hashes[0];
    assert!(stored_hash.starts_with("$argon2id$"));
    assert!(!stored_hash.contains(raw_secret));
}

#[tokio::test]
async fn test_device_api_endpoints_and_websocket_auth() {
    use futures_util::{SinkExt, StreamExt};
    use oxidite_directory::InMemoryDirectory;
    use oxidite_proto::{
        ClientMsg, ErrorCode, PROTOCOL_VERSION, ServerMsg, pack_client_msg, unpack_server_msg,
    };
    use oxidite_server::auth::UserStore;
    use oxidite_server::security::{RateLimiter, SecurityConfig};
    use oxidite_server::share::ShareStore;
    use oxidite_server::{AppState, create_app};
    use oxidite_session::manager::SessionManager;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message;

    let store = DeviceStore::open_in_memory().await.unwrap();
    let bootstrap_token = "bootstrap-admin-token-12345".to_string();
    store.register_bootstrap_grant(&bootstrap_token, "admin");

    let state = AppState {
        manager: SessionManager::new(),
        token: bootstrap_token.clone(),
        static_dir: "web/dist".into(),
        known_hosts: oxidite_ssh::KnownHostsStore::new(),
        security: SecurityConfig::default(),
        auth: UserStore::new(),
        shares: ShareStore::new(),
        rate_limiter: RateLimiter::new(),
        metrics: oxidite_server::metrics::MetricsCollector::new(),
        node_id: "node-dev-auth".into(),
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

    // Helper for HTTP requests
    let http_post = |path: &'static str, body: String, auth: Option<String>| async move {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let mut req = format!(
            "POST {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        );
        if let Some(token) = auth {
            req.push_str(&format!("Authorization: Bearer {token}\r\n"));
        }
        req.push_str("\r\n");
        req.push_str(&body);
        stream.write_all(req.as_bytes()).await.unwrap();

        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        let resp = String::from_utf8_lossy(&buf).to_string();
        let status = resp
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1)?.parse::<u16>().ok())
            .unwrap_or(500);
        let resp_body = resp.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (status, resp_body)
    };

    // 1. Enroll using bootstrap grant
    let enroll_body = format!(r#"{{"grant":"{bootstrap_token}","label":"Admin Workstation"}}"#);
    let (status, body) = http_post("/api/devices/enroll", enroll_body, None).await;
    assert_eq!(status, 200);
    assert!(body.contains("od1."));

    let enroll_json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let device_token = enroll_json["token"].as_str().unwrap().to_string();
    let device_id = enroll_json["device_id"].as_str().unwrap().to_string();

    // 2. Connect via WebSocket using the device token
    let ws_url = format!("ws://{addr}/ws");
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    let hello = pack_client_msg(&ClientMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        token: device_token.clone(),
    })
    .unwrap();
    ws.send(Message::Binary(hello.into())).await.unwrap();

    let msg = ws.next().await.unwrap().unwrap();
    if let Message::Binary(bin) = msg {
        let server_msg = unpack_server_msg(&bin).unwrap();
        assert!(matches!(server_msg, ServerMsg::Welcome { .. }));
    } else {
        panic!("Expected Welcome message");
    }
    let _ = ws.close(None).await;

    // 3. Static dev token bypass rejection test: "your-custom-token" must fail
    let (mut bad_ws, _) = connect_async(&ws_url).await.unwrap();
    let bad_hello = pack_client_msg(&ClientMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        token: "your-custom-token".to_string(),
    })
    .unwrap();
    bad_ws
        .send(Message::Binary(bad_hello.into()))
        .await
        .unwrap();

    let bad_msg = bad_ws.next().await.unwrap().unwrap();
    if let Message::Binary(bin) = bad_msg {
        let server_msg = unpack_server_msg(&bin).unwrap();
        assert!(
            matches!(
                server_msg,
                ServerMsg::Error {
                    code: ErrorCode::Unauthorized,
                    ..
                }
            ),
            "Expected Unauthorized error for dev token bypass"
        );
    } else {
        panic!("Expected error message for dev token bypass");
    }

    // 4. Revoke the device
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "DELETE /api/devices/{device_id} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: Bearer {device_token}\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let resp = String::from_utf8_lossy(&buf).to_string();
    let status = resp
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1)?.parse::<u16>().ok())
        .unwrap_or(500);
    assert_eq!(status, 204);

    // 5. Subsequent WebSocket connection with revoked device token must fail
    let (mut revoked_ws, _) = connect_async(&ws_url).await.unwrap();
    let revoked_hello = pack_client_msg(&ClientMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        token: device_token,
    })
    .unwrap();
    revoked_ws
        .send(Message::Binary(revoked_hello.into()))
        .await
        .unwrap();

    let revoked_msg = revoked_ws.next().await.unwrap().unwrap();
    if let Message::Binary(bin) = revoked_msg {
        let server_msg = unpack_server_msg(&bin).unwrap();
        assert!(matches!(
            server_msg,
            ServerMsg::Error {
                code: ErrorCode::Unauthorized,
                ..
            }
        ));
    } else {
        panic!("Expected Unauthorized error for revoked device");
    }
}
