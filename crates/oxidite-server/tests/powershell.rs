#![allow(clippy::useless_conversion, clippy::field_reassign_with_default)]

use futures_util::{SinkExt, StreamExt};
use oxidite_directory::InMemoryDirectory;
use oxidite_proto::{
    BackendSpec, ClientMsg, ErrorCode, PROTOCOL_VERSION, ServerMsg, ShellCapability, TermSize,
    pack_client_msg, unpack_server_msg,
};
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

async fn spawn_test_server(
    security: SecurityConfig,
) -> (std::net::SocketAddr, String, DeviceStore) {
    let store = DeviceStore::open_in_memory().await.unwrap();
    let token = "bootstrap-admin-token".to_string();
    store.register_bootstrap_grant(&token, "admin");

    let state = AppState {
        manager: SessionManager::new(),
        token: token.clone(),
        static_dir: "web/dist".into(),
        known_hosts: oxidite_ssh::KnownHostsStore::new(),
        security,
        auth: UserStore::new(),
        shares: ShareStore::new(),
        rate_limiter: RateLimiter::new(),
        metrics: oxidite_server::metrics::MetricsCollector::new(),
        node_id: "node-pwsh-test".into(),
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

    (addr, token, store)
}

#[tokio::test]
async fn test_runtime_shells_endpoint_authentication_and_capabilities() {
    let (addr, token, _) = spawn_test_server(SecurityConfig::default()).await;

    // 1. Unauthenticated request should return 401
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(
            b"GET /api/runtime/shells HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let resp = String::from_utf8_lossy(&buf);
    assert!(
        resp.starts_with("HTTP/1.1 401"),
        "Expected 401, got: {resp}"
    );

    // 2. Authenticated request should return 200 with shell capabilities
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "GET /api/runtime/shells HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: Bearer {token}\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let resp = String::from_utf8_lossy(&buf);
    assert!(
        resp.starts_with("HTTP/1.1 200"),
        "Expected 200, got: {resp}"
    );

    let body = resp.split("\r\n\r\n").nth(1).unwrap();
    let capabilities: Vec<ShellCapability> = serde_json::from_str(body.trim()).unwrap();
    assert!(
        capabilities.iter().any(|c| c.id == "SystemPowerShell"),
        "Capabilities must include SystemPowerShell"
    );

    // Capabilities must never reveal executable paths or PATH variables
    for cap in &capabilities {
        assert!(
            !cap.id.contains('\\'),
            "Shell id must not contain backslashes"
        );
        assert!(!cap.id.contains('/'), "Shell id must not contain slashes");
        assert!(
            !cap.id.ends_with(".exe"),
            "Shell id must not reveal file extensions"
        );
    }
}

#[tokio::test]
async fn test_rejection_of_arbitrary_browser_executable_paths() {
    let (addr, token, _) = spawn_test_server(SecurityConfig::default()).await;

    let ws_url = format!("ws://{addr}/ws");
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    let hello = pack_client_msg(&ClientMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        token,
    })
    .unwrap();
    ws.send(Message::Binary(hello.into())).await.unwrap();

    let welcome = ws.next().await.unwrap().unwrap();
    assert!(matches!(
        unpack_server_msg(&welcome.into_data()).unwrap(),
        ServerMsg::Welcome { .. }
    ));

    // Client attempts to execute an arbitrary binary path
    let create = pack_client_msg(&ClientMsg::Create {
        backend: BackendSpec::LocalPty {
            shell: Some("C:\\Windows\\System32\\calc.exe".to_string()),
            cwd: None,
        },
        size: TermSize { rows: 24, cols: 80 },
    })
    .unwrap();
    ws.send(Message::Binary(create.into())).await.unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(3), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    let msg = unpack_server_msg(&resp.into_data()).unwrap();
    match msg {
        ServerMsg::Error { code, message } => {
            assert!(
                code == ErrorCode::InvalidMessage || code == ErrorCode::BackendError,
                "Expected error code for rejected path, got {code:?}"
            );
            assert!(
                message.to_lowercase().contains("arbitrary")
                    || message.to_lowercase().contains("rejected")
                    || message.to_lowercase().contains("not permitted")
                    || message.to_lowercase().contains("invalid"),
                "Expected rejection explanation, got: {message}"
            );
        }
        other => panic!("Expected ServerMsg::Error for arbitrary executable, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_safe_system_powershell_selection_when_available() {
    let (addr, token, _) = spawn_test_server(SecurityConfig::default()).await;

    let ws_url = format!("ws://{addr}/ws");
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    let hello = pack_client_msg(&ClientMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        token,
    })
    .unwrap();
    ws.send(Message::Binary(hello.into())).await.unwrap();

    let _welcome = ws.next().await.unwrap().unwrap();

    // Client requests the safe identifier "SystemPowerShell"
    let create = pack_client_msg(&ClientMsg::Create {
        backend: BackendSpec::LocalPty {
            shell: Some("SystemPowerShell".to_string()),
            cwd: None,
        },
        size: TermSize { rows: 24, cols: 80 },
    })
    .unwrap();
    ws.send(Message::Binary(create.into())).await.unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    let msg = unpack_server_msg(&resp.into_data()).unwrap();
    let pwsh_available = oxidite_pty::is_powershell_available();

    if pwsh_available {
        match msg {
            ServerMsg::Created { session } => {
                assert!(!session.is_nil());
            }
            other => panic!("Expected Created for available SystemPowerShell, got: {other:?}"),
        }
    } else {
        match msg {
            ServerMsg::Error { code, message } => {
                assert_eq!(code, ErrorCode::BackendError);
                assert!(message.contains("PowerShell"));
            }
            other => panic!("Expected Error for absent SystemPowerShell, got: {other:?}"),
        }
    }
}

#[tokio::test]
async fn test_local_pty_disabled_on_non_loopback_bind() {
    let mut sec = SecurityConfig::default();
    sec.allow_non_loopback_pty = false;
    sec.bind_addr = Some("192.168.1.100:3000".parse().unwrap());

    let (addr, token, _) = spawn_test_server(sec).await;

    let ws_url = format!("ws://{addr}/ws");
    let (mut ws, _) = connect_async(&ws_url).await.unwrap();

    let hello = pack_client_msg(&ClientMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        token,
    })
    .unwrap();
    ws.send(Message::Binary(hello)).await.unwrap();

    let _welcome = ws.next().await.unwrap().unwrap();

    // Client requests LocalPty on non-loopback bind without allow_non_loopback_pty
    let create = pack_client_msg(&ClientMsg::Create {
        backend: BackendSpec::LocalPty {
            shell: None,
            cwd: None,
        },
        size: TermSize { rows: 24, cols: 80 },
    })
    .unwrap();
    ws.send(Message::Binary(create)).await.unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(3), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    let msg = unpack_server_msg(&resp.into_data()).unwrap();
    match msg {
        ServerMsg::Error { code, message } => {
            assert_eq!(code, ErrorCode::Unauthorized);
            assert!(message.to_lowercase().contains("loopback"));
        }
        other => panic!("Expected Unauthorized error on non-loopback bind, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_runtime_shells_disabled_on_non_loopback_bind() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut sec = SecurityConfig::default();
    sec.allow_non_loopback_pty = false;
    sec.bind_addr = Some("192.168.1.100:3000".parse().unwrap());

    let (addr, token, _) = spawn_test_server(sec).await;
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "GET /api/runtime/shells HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).await.unwrap();

    let body = resp.split("\r\n\r\n").nth(1).expect("HTTP body");
    let shells: Vec<serde_json::Value> = serde_json::from_str(body).unwrap();
    assert_eq!(shells.len(), 2);
    assert!(
        shells
            .iter()
            .all(|s| s["available"] == serde_json::Value::Bool(false))
    );
}
