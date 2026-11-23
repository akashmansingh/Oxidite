//! Prometheus metrics exporter, health, readiness, and tracing spans for Oxidite.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{Span, info_span};

use crate::AppState;
use crate::routing::RouteClass;

#[derive(Default, Clone)]
pub struct MetricsCollector {
    pub reconnects_total: Arc<AtomicU64>,
    pub bytes_transferred_total: Arc<AtomicU64>,
    pub replay_lag_bytes: Arc<AtomicU64>,
    pub http_requests: Arc<Mutex<BTreeMap<(RouteClass, u16), u64>>>,
    pub http_response_bytes: Arc<Mutex<BTreeMap<RouteClass, u64>>>,
    pub ws_handshakes_total: Arc<AtomicU64>,
    pub ws_handshake_duration_micros_total: Arc<AtomicU64>,
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn inc_reconnects(&self) {
        self.reconnects_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn add_bytes(&self, count: u64) {
        self.bytes_transferred_total
            .fetch_add(count, Ordering::Relaxed);
    }

    pub fn set_replay_lag(&self, lag: u64) {
        self.replay_lag_bytes.store(lag, Ordering::Relaxed);
    }

    /// Observes an HTTP request with low-cardinality route class and status code.
    pub fn observe_http(
        &self,
        class: RouteClass,
        status: StatusCode,
        _duration: Duration,
        bytes: u64,
    ) {
        let status_code = status.as_u16();
        if let Ok(mut reqs) = self.http_requests.lock() {
            *reqs.entry((class, status_code)).or_insert(0) += 1;
        }
        if bytes > 0 {
            if let Ok(mut resp_bytes) = self.http_response_bytes.lock() {
                *resp_bytes.entry(class).or_insert(0) += bytes;
            }
        }
    }

    /// Observes a WebSocket handshake completion.
    pub fn observe_websocket_handshake(&self, duration: Duration) {
        self.ws_handshakes_total.fetch_add(1, Ordering::Relaxed);
        let micros = duration.as_micros() as u64;
        self.ws_handshake_duration_micros_total
            .fetch_add(micros, Ordering::Relaxed);
    }

    /// Renders all metrics in standard Prometheus exposition text format.
    pub fn render_prometheus(&self, active_sessions: usize) -> String {
        let mut out = format!(
            "# HELP oxidite_active_sessions Number of active sessions\n\
             # TYPE oxidite_active_sessions gauge\n\
             oxidite_active_sessions {active_sessions}\n\
             # HELP oxidite_reconnects_total Total number of reconnect attempts\n\
             # TYPE oxidite_reconnects_total counter\n\
             oxidite_reconnects_total {}\n\
             # HELP oxidite_bytes_transferred_total Total output bytes streamed\n\
             # TYPE oxidite_bytes_transferred_total counter\n\
             oxidite_bytes_transferred_total {}\n\
             # HELP oxidite_replay_lag_bytes Current replay buffer lag in bytes\n\
             # TYPE oxidite_replay_lag_bytes gauge\n\
             oxidite_replay_lag_bytes {}\n",
            self.reconnects_total.load(Ordering::Relaxed),
            self.bytes_transferred_total.load(Ordering::Relaxed),
            self.replay_lag_bytes.load(Ordering::Relaxed),
        );

        if let Ok(http_requests) = self.http_requests.lock() {
            if !http_requests.is_empty() {
                out.push_str("# HELP oxidite_http_requests_total Total HTTP requests by route class and status\n");
                out.push_str("# TYPE oxidite_http_requests_total counter\n");
                for ((route_class, status), count) in http_requests.iter() {
                    out.push_str(&format!(
                        "oxidite_http_requests_total{{route_class=\"{}\",status=\"{}\"}} {}\n",
                        route_class.as_label(),
                        status,
                        count
                    ));
                }
            }
        }

        if let Ok(http_bytes) = self.http_response_bytes.lock() {
            if !http_bytes.is_empty() {
                out.push_str("# HELP oxidite_http_response_bytes_total Total HTTP response bytes by route class\n");
                out.push_str("# TYPE oxidite_http_response_bytes_total counter\n");
                for (route_class, bytes) in http_bytes.iter() {
                    out.push_str(&format!(
                        "oxidite_http_response_bytes_total{{route_class=\"{}\"}} {}\n",
                        route_class.as_label(),
                        bytes
                    ));
                }
            }
        }

        out.push_str("# HELP oxidite_websocket_handshakes_total Total WebSocket handshakes\n");
        out.push_str("# TYPE oxidite_websocket_handshakes_total counter\n");
        out.push_str(&format!(
            "oxidite_websocket_handshakes_total {}\n",
            self.ws_handshakes_total.load(Ordering::Relaxed)
        ));

        let ws_micros = self
            .ws_handshake_duration_micros_total
            .load(Ordering::Relaxed);
        let ws_seconds = ws_micros as f64 / 1_000_000.0;
        out.push_str("# HELP oxidite_websocket_handshake_duration_seconds_total Total duration of WebSocket handshakes in seconds\n");
        out.push_str("# TYPE oxidite_websocket_handshake_duration_seconds_total counter\n");
        out.push_str(&format!(
            "oxidite_websocket_handshake_duration_seconds_total {:.6}\n",
            ws_seconds
        ));

        out
    }
}

/// Handler for `GET /metrics` exporting Prometheus metrics.
pub async fn prometheus_metrics_handler(State(state): State<AppState>) -> Response {
    let active_sessions = state.manager.list().len();
    let text = state.metrics.render_prometheus(active_sessions);
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        text,
    )
        .into_response()
}

/// Handler for `GET /ready` verifying directory and node readiness.
pub async fn readiness_handler(State(state): State<AppState>) -> Response {
    match state.directory.list_orphans().await {
        Ok(_) => (StatusCode::OK, "READY").into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "NOT_READY: directory unreachable",
        )
            .into_response(),
    }
}

/// Creates a tracing span for a session.
pub fn session_span(session_id: &str, backend: &str) -> Span {
    info_span!("session", id = %session_id, backend = %backend)
}
