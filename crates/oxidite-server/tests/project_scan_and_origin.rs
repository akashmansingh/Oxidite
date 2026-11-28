//! Integration tests for `/api/project/scan` bounded scanning & workspace-root confinement,
//! plus `OriginPolicy` exact `.ts.net` validation & loopback-only `X-Forwarded-*` trust.

use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::http::{HeaderMap, HeaderValue, header};
use oxidite_server::auth::UserStore;
use oxidite_server::security::{RateLimiter, SecurityConfig};
use oxidite_server::share::ShareStore;
use oxidite_server::{AppState, create_app};
use oxidite_session::manager::SessionManager;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn scan_project_dir_handles_missing_path_cleanly() {
    let missing = std::env::temp_dir().join("oxidite_definitely_does_not_exist_9999");
    let res = oxidite_server::project_scan::scan_project_dir(missing).await;
    assert!(res.is_err());
}

#[tokio::test]
async fn scan_project_dir_runs_async_and_returns_uppercase_wordmark() {
    let dir = std::env::temp_dir()
        .join(format!("oxidite_int_{}", uuid::Uuid::new_v4()))
        .join("my-service");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("package.json"),
        r#"{"name":"my-service","dependencies":{"react":"19.0.0"}}"#,
    )
    .unwrap();
    fs::write(dir.join("tsconfig.json"), "{}").unwrap();

    let report = oxidite_server::project_scan::scan_project_dir(dir.clone())
        .await
        .expect("scan should succeed");

    assert_eq!(report.wordmark, "MY-SERVICE");
    assert_eq!(report.language.as_deref(), Some("TypeScript"));
    assert!(report.frameworks.contains(&"React".to_string()));
    assert_eq!(report.files, 2);

    let _ = fs::remove_dir_all(dir.parent().unwrap());
}

#[tokio::test]
async fn api_project_scan_enforces_auth_and_workspace_roots() {
    let temp_root =
        std::env::temp_dir().join(format!("oxidite_http_scan_{}", uuid::Uuid::new_v4()));
    let allowed_dir = temp_root.join("repo");
    let outside_dir = temp_root.join("outside");
    fs::create_dir_all(&allowed_dir).unwrap();
    fs::create_dir_all(&outside_dir).unwrap();
    fs::write(
        allowed_dir.join("package.json"),
        r#"{"name":"repo","dependencies":{"vite":"^6.0.0"}}"#,
    )
    .unwrap();
    fs::write(allowed_dir.join("tsconfig.json"), "{}").unwrap();

    let manager = SessionManager::new();
    let ok_sid = uuid::Uuid::new_v4();
    let blocked_sid = uuid::Uuid::new_v4();
    manager.set_cwd(ok_sid, allowed_dir.clone());
    manager.set_cwd(blocked_sid, outside_dir.clone());

    let sec = SecurityConfig {
        workspace_roots: vec![allowed_dir.clone()],
        ..SecurityConfig::default()
    };

    let token = "scan-api-token-1".to_string();
    let state = AppState {
        manager,
        token: token.clone(),
        static_dir: "web/dist".into(),
        known_hosts: oxidite_ssh::KnownHostsStore::new(),
        security: sec,
        auth: UserStore::new(),
        shares: ShareStore::new(),
        rate_limiter: RateLimiter::new(),
        metrics: oxidite_server::metrics::MetricsCollector::new(),
        node_id: "node-scan-test".into(),
        directory: std::sync::Arc::new(oxidite_directory::InMemoryDirectory::new()),
        devices: oxidite_server::devices::DeviceStore::default(),
        device_connections: oxidite_server::device_connections::DeviceConnectionRegistry::default(),
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, create_app(state)).await.unwrap();
    });

    async fn http_get(addr: SocketAddr, path: &str, auth: &str) -> (u16, String) {
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {auth}\r\nConnection: close\r\n\r\n"
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut raw = String::new();
        s.read_to_string(&mut raw).await.unwrap();
        let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
        let code = head
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|c| c.parse::<u16>().ok())
            .unwrap_or(0);
        (code, body.to_string())
    }

    let (st_unauth, _) = http_get(addr, &format!("/api/project/scan?sid={ok_sid}"), "wrong").await;
    assert_eq!(st_unauth, 401);

    let (st_blocked, _) = http_get(
        addr,
        &format!("/api/project/scan?sid={blocked_sid}"),
        &token,
    )
    .await;
    assert_eq!(st_blocked, 403);

    let (st_ok, body) = http_get(addr, &format!("/api/project/scan?sid={ok_sid}"), &token).await;
    assert_eq!(st_ok, 200);
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["wordmark"], "REPO");
    assert_eq!(parsed["language"], "TypeScript");

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn origin_policy_rejects_spoofed_tailscale_and_non_loopback_forwarded_headers() {
    let policy = oxidite_server::origin_guard::OriginPolicy::from_csv(
        "https://victus.tail123.ts.net, https://*.ts.net",
    );

    let mut ok_headers = HeaderMap::new();
    ok_headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("https://victus.tail123.ts.net"),
    );
    assert!(policy.is_origin_allowed(&ok_headers));

    let mut evil_headers = HeaderMap::new();
    evil_headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("https://evil.ts.net"),
    );
    assert!(!policy.is_origin_allowed(&evil_headers));

    let mut fwd = HeaderMap::new();
    fwd.insert(
        "x-forwarded-host",
        HeaderValue::from_static("victus.tail123.ts.net"),
    );
    fwd.insert("x-forwarded-proto", HeaderValue::from_static("https"));

    let loopback = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4444);
    let lan = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)), 4444);

    assert_eq!(
        policy.external_base_url(loopback, &fwd, "127.0.0.1:3030"),
        "https://victus.tail123.ts.net"
    );
    assert_eq!(
        policy.external_base_url(lan, &fwd, "127.0.0.1:3030"),
        "http://127.0.0.1:3030"
    );
}
