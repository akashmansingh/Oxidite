//! Prometheus metrics exporter, health, readiness, and tracing spans for Oxidite.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{Span, info_span};

use crate::AppState;

#[derive(Default, Clone)]
pub struct MetricsCollector {
    pub reconnects_total: Arc<AtomicU64>,
    pub bytes_transferred_total: Arc<AtomicU64>,
    pub replay_lag_bytes: Arc<AtomicU64>,
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

    /// Renders all metrics in standard Prometheus exposition text format.
    pub fn render_prometheus(&self, active_sessions: usize) -> String {
        format!(
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
        )
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
