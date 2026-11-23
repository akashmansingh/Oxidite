use oxidite_directory::{InMemoryDirectory, PartitionableDirectory};
use oxidite_server::auth::UserStore;
use oxidite_server::metrics::MetricsCollector;
use oxidite_server::security::{RateLimiter, SecurityConfig};
use oxidite_server::share::ShareStore;
use oxidite_server::{AppState, create_app};
use oxidite_session::manager::SessionManager;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

async fn http_get(addr: std::net::SocketAddr, path: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.unwrap();

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let resp = String::from_utf8_lossy(&buf).to_string();

    let status_code = resp
        .lines()
        .next()
        .and_then(|line| {
            let mut parts = line.split_whitespace();
            parts.next(); // HTTP/1.1
            parts.next()?.parse::<u16>().ok()
        })
        .unwrap_or(500);

    let body = resp.split("\r\n\r\n").nth(1).unwrap_or("").to_string();

    (status_code, body)
}

#[tokio::test]
async fn test_prometheus_metrics_and_readiness_endpoints() {
    let raw_dir = InMemoryDirectory::new();
    let part_dir = PartitionableDirectory::new(raw_dir);

    let metrics = MetricsCollector::new();
    metrics.inc_reconnects();
    metrics.add_bytes(2048);

    let state = AppState {
        manager: SessionManager::new(),
        token: "test-token".into(),
        static_dir: "web/dist".into(),
        known_hosts: oxidite_ssh::KnownHostsStore::new(),
        security: SecurityConfig::default(),
        auth: UserStore::new(),
        shares: ShareStore::new(),
        rate_limiter: RateLimiter::new(),
        metrics: metrics.clone(),
        node_id: "node-obs-1".into(),
        directory: Arc::new(part_dir.clone()),
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let app = create_app(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // 1. Check /metrics
    let (status, body) = http_get(addr, "/metrics").await;
    assert_eq!(status, 200);
    assert!(body.contains("oxidite_active_sessions 0"));
    assert!(body.contains("oxidite_reconnects_total 1"));
    assert!(body.contains("oxidite_bytes_transferred_total 2048"));

    // 2. Check /ready when healthy
    let (status, body) = http_get(addr, "/ready").await;
    assert_eq!(status, 200);
    assert_eq!(body, "READY");

    // 3. Inject partition -> /ready must return 503
    part_dir.set_partitioned(true);
    let (status, body) = http_get(addr, "/ready").await;
    assert_eq!(status, 503);
    assert!(body.contains("NOT_READY"));
}

#[tokio::test]
async fn test_route_class_metrics_contain_no_raw_identifiers() {
    use axum::http::StatusCode;
    use oxidite_server::routing::RouteClass;
    use std::time::Duration;

    let metrics = MetricsCollector::new();

    // Observe various HTTP routes and WebSocket handshake
    metrics.observe_http(
        RouteClass::Api,
        StatusCode::OK,
        Duration::from_millis(15),
        256,
    );
    metrics.observe_http(
        RouteClass::FingerprintedAsset,
        StatusCode::OK,
        Duration::from_millis(5),
        1024,
    );
    metrics.observe_http(
        RouteClass::Html,
        StatusCode::OK,
        Duration::from_millis(8),
        512,
    );
    metrics.observe_http(
        RouteClass::Static,
        StatusCode::NOT_FOUND,
        Duration::from_millis(2),
        0,
    );
    metrics.observe_websocket_handshake(Duration::from_millis(12));

    let rendered = metrics.render_prometheus(0);

    // Verify low-cardinality route-class metrics exist
    assert!(rendered.contains("oxidite_http_requests_total{route_class=\"api\",status=\"200\"} 1"));
    assert!(rendered.contains(
        "oxidite_http_requests_total{route_class=\"fingerprinted_asset\",status=\"200\"} 1"
    ));
    assert!(
        rendered.contains("oxidite_http_requests_total{route_class=\"html\",status=\"200\"} 1")
    );
    assert!(
        rendered.contains("oxidite_http_requests_total{route_class=\"static\",status=\"404\"} 1")
    );
    assert!(rendered.contains("oxidite_http_response_bytes_total{route_class=\"api\"} 256"));
    assert!(
        rendered.contains(
            "oxidite_http_response_bytes_total{route_class=\"fingerprinted_asset\"} 1024"
        )
    );
    assert!(rendered.contains("oxidite_websocket_handshakes_total 1"));
    assert!(rendered.contains("oxidite_websocket_handshake_duration_seconds_total"));

    // Verify REDACTION: No paths, query strings, user IDs, or tokens in metrics output
    assert!(!rendered.contains("/api/"));
    assert!(!rendered.contains("/assets/"));
    assert!(!rendered.contains("token="));
    assert!(!rendered.contains("user_id="));
    assert!(!rendered.contains("session_id="));
    assert!(!rendered.contains("device_id="));
}
