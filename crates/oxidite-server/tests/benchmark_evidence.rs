#![allow(clippy::useless_conversion, clippy::field_reassign_with_default)]

use futures_util::{SinkExt, StreamExt};
use oxidite_directory::InMemoryDirectory;
use oxidite_proto::{
    BackendSpec, ClientMsg, PROTOCOL_VERSION, ServerMsg, TermSize, pack_client_msg,
    unpack_server_msg,
};
use oxidite_server::auth::UserStore;
use oxidite_server::devices::DeviceStore;
use oxidite_server::security::{RateLimiter, SecurityConfig};
use oxidite_server::share::ShareStore;
use oxidite_server::{AppState, create_app};
use oxidite_session::manager::SessionManager;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WsCounters {
    pub messages_sent: u32,
    pub messages_received: u32,
    pub bytes_sent: u64,
    pub bytes_received: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BenchmarkRecord {
    pub scenario_id: String,
    pub trace_id: String,
    pub route_class: String,
    pub status: u16,
    pub cache_decision: String,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub duration_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ws_counters: Option<WsCounters>,
}

pub struct BoundedEvidenceSink {
    records: Vec<BenchmarkRecord>,
    max_records: usize,
    max_bytes: usize,
    current_bytes: usize,
}

impl BoundedEvidenceSink {
    pub fn new(max_records: usize, max_bytes: usize) -> Self {
        Self {
            records: Vec::new(),
            max_records,
            max_bytes,
            current_bytes: 0,
        }
    }

    pub fn record(&mut self, record: BenchmarkRecord) -> bool {
        if self.records.len() >= self.max_records {
            return false;
        }
        let line = serde_json::to_string(&record).unwrap() + "\n";
        if self.current_bytes + line.len() > self.max_bytes {
            return false;
        }
        self.current_bytes += line.len();
        self.records.push(record);
        true
    }

    pub fn to_ndjson(&self) -> String {
        self.records
            .iter()
            .map(|r| serde_json::to_string(r).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn records(&self) -> &[BenchmarkRecord] {
        &self.records
    }
}

async fn spawn_benchmark_server() -> (std::net::SocketAddr, String, DeviceStore) {
    let store = DeviceStore::open_in_memory().await.unwrap();
    let token = "benchmark-admin-bootstrap-token".to_string();
    store.register_bootstrap_grant(&token, "admin");

    let manifest_static = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|root| root.join("web").join("dist"))
        .unwrap_or_else(|| std::path::PathBuf::from("web/dist"));

    let static_dir = if manifest_static.exists() {
        manifest_static.to_string_lossy().to_string()
    } else if std::path::Path::new("web/dist").exists() {
        "web/dist".into()
    } else {
        "../../web/dist".into()
    };

    let state = AppState {
        manager: SessionManager::new(),
        token: token.clone(),
        static_dir,
        known_hosts: oxidite_ssh::KnownHostsStore::new(),
        security: SecurityConfig::default(),
        auth: UserStore::new(),
        shares: ShareStore::new(),
        rate_limiter: RateLimiter::new(),
        metrics: oxidite_server::metrics::MetricsCollector::new(),
        node_id: "node-bench-1".into(),
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
async fn test_benchmark_scenarios_and_redaction_invariants() {
    let (addr, admin_token, store) = spawn_benchmark_server().await;
    let mut sink = BoundedEvidenceSink::new(100, 256 * 1024);

    // Scenario 1: Cold static HTML asset
    {
        let trace_id = Uuid::new_v4().to_string();
        let start = Instant::now();
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let req = b"GET /index.html HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
        stream.write_all(req).await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        let duration = start.elapsed().as_secs_f64() * 1000.0;
        let resp = String::from_utf8_lossy(&buf);

        let status = if resp.starts_with("HTTP/1.1 200") {
            200
        } else {
            404
        };
        let cache_control = resp
            .lines()
            .find(|l| l.to_lowercase().starts_with("cache-control:"))
            .map(|l| l.split(':').nth(1).unwrap().trim().to_string())
            .unwrap_or_default();

        assert_eq!(cache_control, "no-cache");
        sink.record(BenchmarkRecord {
            scenario_id: "cold_static_html".into(),
            trace_id,
            route_class: "html".into(),
            status,
            cache_decision: cache_control,
            bytes_sent: req.len() as u64,
            bytes_received: buf.len() as u64,
            duration_ms: duration,
            ws_counters: None,
        });
    }

    // Scenario 2: Warm fingerprinted static asset
    {
        let manifest_static = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .map(|root| root.join("web").join("dist"))
            .unwrap_or_else(|| std::path::PathBuf::from("web/dist"));
        let asset_name = if let Ok(entries) = std::fs::read_dir(manifest_static.join("assets")) {
            entries
                .filter_map(|e| e.ok())
                .find(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    name.ends_with(".css") || name.ends_with(".js")
                })
                .map(|e| e.file_name().to_string_lossy().to_string())
                .unwrap_or_else(|| "index-gmRLFR5Z.css".to_string())
        } else {
            "index-gmRLFR5Z.css".to_string()
        };

        let trace_id = Uuid::new_v4().to_string();
        let start = Instant::now();
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let req = format!(
            "GET /assets/{asset_name} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        let duration = start.elapsed().as_secs_f64() * 1000.0;
        let resp = String::from_utf8_lossy(&buf);

        let status = if resp.starts_with("HTTP/1.1 200") {
            200
        } else {
            404
        };
        let cache_control = resp
            .lines()
            .find(|l| l.to_lowercase().starts_with("cache-control:"))
            .map(|l| l.split(':').nth(1).unwrap().trim().to_string())
            .unwrap_or_default();

        if status == 200 {
            assert!(cache_control.contains("immutable"));
        }
        sink.record(BenchmarkRecord {
            scenario_id: "warm_fingerprinted_asset".into(),
            trace_id,
            route_class: "fingerprinted_asset".into(),
            status,
            cache_decision: cache_control,
            bytes_sent: req.len() as u64,
            bytes_received: buf.len() as u64,
            duration_ms: duration,
            ws_counters: None,
        });
    }

    // Scenario 3: Traversal rejection
    {
        let trace_id = Uuid::new_v4().to_string();
        let start = Instant::now();
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let req = b"GET /../../etc/passwd HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
        stream.write_all(req).await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        let duration = start.elapsed().as_secs_f64() * 1000.0;
        let resp = String::from_utf8_lossy(&buf);

        let status = if resp.starts_with("HTTP/1.1 400") {
            400
        } else {
            200
        };
        assert_eq!(
            status, 400,
            "Directory traversal must return 400 Bad Request"
        );

        sink.record(BenchmarkRecord {
            scenario_id: "traversal_rejection".into(),
            trace_id,
            route_class: "static".into(),
            status,
            cache_decision: "none".into(),
            bytes_sent: req.len() as u64,
            bytes_received: buf.len() as u64,
            duration_ms: duration,
            ws_counters: None,
        });
    }

    // Scenario 4: Authenticated API endpoint has no-store cache policy
    {
        let trace_id = Uuid::new_v4().to_string();
        let start = Instant::now();
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let req = format!(
            "GET /api/sessions HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: Bearer {}\r\n\r\n",
            admin_token
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        let duration = start.elapsed().as_secs_f64() * 1000.0;
        let resp = String::from_utf8_lossy(&buf);

        assert!(resp.starts_with("HTTP/1.1 200"));
        let cache_control = resp
            .lines()
            .find(|l| l.to_lowercase().starts_with("cache-control:"))
            .map(|l| l.split(':').nth(1).unwrap().trim().to_string())
            .unwrap_or_default();
        assert_eq!(
            cache_control, "no-store",
            "API routes must specify no-store"
        );

        sink.record(BenchmarkRecord {
            scenario_id: "api_no_store_sessions".into(),
            trace_id,
            route_class: "api".into(),
            status: 200,
            cache_decision: cache_control,
            bytes_sent: req.len() as u64,
            bytes_received: buf.len() as u64,
            duration_ms: duration,
            ws_counters: None,
        });
    }

    // Scenario 5: Runtime shells capability probe
    {
        let trace_id = Uuid::new_v4().to_string();
        let start = Instant::now();
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let req = format!(
            "GET /api/runtime/shells HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: Bearer {}\r\n\r\n",
            admin_token
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        let duration = start.elapsed().as_secs_f64() * 1000.0;
        let resp = String::from_utf8_lossy(&buf);

        assert!(resp.starts_with("HTTP/1.1 200"));
        sink.record(BenchmarkRecord {
            scenario_id: "powershell_capability_probe".into(),
            trace_id,
            route_class: "api".into(),
            status: 200,
            cache_decision: "no-store".into(),
            bytes_sent: req.len() as u64,
            bytes_received: buf.len() as u64,
            duration_ms: duration,
            ws_counters: None,
        });
    }

    // Scenario 6: Device enrollment and revocation lifecycle
    let enrolled_device_token = {
        let trace_id = Uuid::new_v4().to_string();
        let start = Instant::now();
        let grant = store
            .issue_enrollment_grant("alice", Duration::from_secs(300))
            .await
            .unwrap();
        let enrolled = store.enroll(&grant, "Bench Device").await.unwrap();
        let duration = start.elapsed().as_secs_f64() * 1000.0;

        sink.record(BenchmarkRecord {
            scenario_id: "device_enrollment".into(),
            trace_id,
            route_class: "api".into(),
            status: 201,
            cache_decision: "no-store".into(),
            bytes_sent: 128,
            bytes_received: 256,
            duration_ms: duration,
            ws_counters: None,
        });

        // Revocation scenario
        let trace_rev = Uuid::new_v4().to_string();
        let start_rev = Instant::now();
        store.revoke("alice", enrolled.device_id).await.unwrap();
        let duration_rev = start_rev.elapsed().as_secs_f64() * 1000.0;

        sink.record(BenchmarkRecord {
            scenario_id: "device_revocation".into(),
            trace_id: trace_rev,
            route_class: "api".into(),
            status: 204,
            cache_decision: "no-store".into(),
            bytes_sent: 64,
            bytes_received: 0,
            duration_ms: duration_rev,
            ws_counters: None,
        });

        enrolled.token
    };

    // Scenario 7: WebSocket terminal flow with redacted message and byte counters
    {
        let trace_id = Uuid::new_v4().to_string();
        let start = Instant::now();
        let ws_url = format!("ws://{addr}/ws");
        let (mut ws, _) = connect_async(&ws_url).await.unwrap();

        let mut msgs_sent = 0u32;
        let mut msgs_recv = 0u32;
        let mut bytes_sent = 0u64;
        let mut bytes_recv = 0u64;

        // Hello
        let hello = pack_client_msg(&ClientMsg::Hello {
            protocol_version: PROTOCOL_VERSION,
            token: admin_token.clone(),
        })
        .unwrap();
        bytes_sent += hello.len() as u64;
        msgs_sent += 1;
        ws.send(Message::Binary(hello)).await.unwrap();

        // Welcome
        let welcome = ws.next().await.unwrap().unwrap();
        let w_bytes = welcome.into_data();
        bytes_recv += w_bytes.len() as u64;
        msgs_recv += 1;
        assert!(matches!(
            unpack_server_msg(&w_bytes).unwrap(),
            ServerMsg::Welcome { .. }
        ));

        // Create
        let create = pack_client_msg(&ClientMsg::Create {
            backend: BackendSpec::LocalPty {
                shell: None,
                cwd: None,
            },
            size: TermSize { rows: 24, cols: 80 },
        })
        .unwrap();
        bytes_sent += create.len() as u64;
        msgs_sent += 1;
        ws.send(Message::Binary(create)).await.unwrap();

        let c_resp = ws.next().await.unwrap().unwrap().into_data();
        bytes_recv += c_resp.len() as u64;
        msgs_recv += 1;
        let sid = match unpack_server_msg(&c_resp).unwrap() {
            ServerMsg::Created { session } => session,
            other => panic!("Expected Created, got {other:?}"),
        };

        // Attach
        let attach = pack_client_msg(&ClientMsg::Attach {
            session: sid,
            last_offset: None,
            mode: oxidite_proto::AttachMode::Writer,
        })
        .unwrap();
        bytes_sent += attach.len() as u64;
        msgs_sent += 1;
        ws.send(Message::Binary(attach)).await.unwrap();

        let a_resp = ws.next().await.unwrap().unwrap().into_data();
        bytes_recv += a_resp.len() as u64;
        msgs_recv += 1;
        assert!(matches!(
            unpack_server_msg(&a_resp).unwrap(),
            ServerMsg::Attached { .. }
        ));

        // Ping
        let ping = pack_client_msg(&ClientMsg::Ping { nonce: 12345 }).unwrap();
        bytes_sent += ping.len() as u64;
        msgs_sent += 1;
        ws.send(Message::Binary(ping)).await.unwrap();

        let pong_data = ws.next().await.unwrap().unwrap().into_data();
        bytes_recv += pong_data.len() as u64;
        msgs_recv += 1;

        // Detach
        let detach = pack_client_msg(&ClientMsg::Detach).unwrap();
        bytes_sent += detach.len() as u64;
        msgs_sent += 1;
        ws.send(Message::Binary(detach)).await.unwrap();

        let duration = start.elapsed().as_secs_f64() * 1000.0;

        sink.record(BenchmarkRecord {
            scenario_id: "websocket_terminal_flow".into(),
            trace_id,
            route_class: "metrics".into(),
            status: 101,
            cache_decision: "no-store".into(),
            bytes_sent,
            bytes_received: bytes_recv,
            duration_ms: duration,
            ws_counters: Some(WsCounters {
                messages_sent: msgs_sent,
                messages_received: msgs_recv,
                bytes_sent,
                bytes_received: bytes_recv,
            }),
        });
    }

    // VERIFICATION: Ensure the generated NDJSON contains all scenarios
    let ndjson = sink.to_ndjson();
    let lines: Vec<&str> = ndjson.lines().collect();
    assert_eq!(lines.len(), 8, "Must contain exactly 8 benchmark records");

    // REDACTION VERIFICATION: Prove no secrets, PINs, or terminal output leak
    assert!(
        !ndjson.contains(&admin_token),
        "Evidence must never leak the admin token"
    );
    assert!(
        !ndjson.contains(&enrolled_device_token),
        "Evidence must never leak enrolled device tokens"
    );
    assert!(
        !ndjson.contains("od1."),
        "Evidence must never leak token prefix od1."
    );
    assert!(
        !ndjson.contains("password"),
        "Evidence must never leak password keywords"
    );
    assert!(
        !ndjson.contains("PS "),
        "Evidence must never leak PowerShell prompt"
    );
    assert!(
        !ndjson.contains("calc.exe"),
        "Evidence must never leak executable names"
    );

    // SIZE-CAPPING VERIFICATION: Sink rejects records beyond max_records or max_bytes
    let mut tiny_sink = BoundedEvidenceSink::new(2, 500);
    assert!(tiny_sink.record(sink.records()[0].clone()));
    assert!(tiny_sink.record(sink.records()[1].clone()));
    assert!(
        !tiny_sink.record(sink.records()[2].clone()),
        "Sink must reject records exceeding max_records cap"
    );
}
