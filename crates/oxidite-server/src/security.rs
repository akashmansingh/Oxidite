//! Security controls: CSP headers, Origin checking, audit logging, rate limiting, and limits enforcement.

use axum::{
    extract::Request,
    http::{HeaderValue, header},
    middleware::Next,
    response::Response,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::info;

#[derive(Debug, Clone)]
pub struct SecurityConfig {
    pub allowed_origins: Vec<String>,
    pub allow_non_loopback_pty: bool,
    pub max_message_size: usize,
    pub max_sessions_per_user: usize,
    pub max_replay_buffer: usize,
    pub rate_limit_per_minute: u32,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            allowed_origins: vec![
                "http://localhost:3000".into(),
                "http://127.0.0.1:3000".into(),
            ],
            allow_non_loopback_pty: false,
            max_message_size: 64 * 1024,
            max_sessions_per_user: 10,
            max_replay_buffer: 8 * 1024 * 1024,
            rate_limit_per_minute: 120,
        }
    }
}

impl SecurityConfig {
    #[must_use]
    pub fn is_origin_allowed(&self, origin: Option<&str>) -> bool {
        match origin {
            None => true, // Same-origin or non-browser client (curl, test runner)
            Some(o) => {
                self.allowed_origins.iter().any(|allowed| allowed == o)
                    || o.starts_with("http://127.0.0.1:")
                    || o.starts_with("http://localhost:")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum AuditEvent<'a> {
    LoginAttempt {
        username: &'a str,
        success: bool,
        remote_ip: &'a str,
    },
    SessionCreated {
        session_id: &'a str,
        backend_kind: &'a str,
        user: &'a str,
    },
    SessionAttached {
        session_id: &'a str,
        mode: &'a str,
        user: &'a str,
    },
    SessionTerminated {
        session_id: &'a str,
        user: &'a str,
    },
    DeviceEnrolled {
        device_id: &'a str,
        user: &'a str,
    },
    DeviceAuthenticated {
        device_id: &'a str,
        user: &'a str,
    },
    SecurityViolation {
        reason: &'a str,
        remote_ip: &'a str,
    },
}

pub struct AuditLogger;

impl AuditLogger {
    pub fn record(event: AuditEvent<'_>) {
        // Invariant: Never log keystrokes, passwords, output bytes, tokens, or private keys.
        match event {
            AuditEvent::LoginAttempt {
                username,
                success,
                remote_ip,
            } => {
                info!(target: "audit", action = "login", username, success, remote_ip);
            }
            AuditEvent::SessionCreated {
                session_id,
                backend_kind,
                user,
            } => {
                info!(target: "audit", action = "create_session", session_id, backend_kind, user);
            }
            AuditEvent::SessionAttached {
                session_id,
                mode,
                user,
            } => {
                info!(target: "audit", action = "attach_session", session_id, mode, user);
            }
            AuditEvent::SessionTerminated { session_id, user } => {
                info!(target: "audit", action = "terminate_session", session_id, user);
            }
            AuditEvent::DeviceEnrolled { device_id, user } => {
                info!(target: "audit", action = "device_enrolled", device_id, user);
            }
            AuditEvent::DeviceAuthenticated { device_id, user } => {
                info!(target: "audit", action = "device_authenticated", device_id, user);
            }
            AuditEvent::SecurityViolation { reason, remote_ip } => {
                info!(target: "audit", action = "security_violation", reason, remote_ip);
            }
        }
    }
}

/// Simple thread-safe sliding window rate limiter.
#[derive(Clone, Default)]
pub struct RateLimiter {
    counts: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks if a client identifier is within its rate limit for a given window.
    pub fn check_limit(&self, client_id: &str, max_requests: u32, window: Duration) -> bool {
        let mut map = self.counts.lock().unwrap();
        let now = Instant::now();
        let timestamps = map.entry(client_id.to_string()).or_default();
        timestamps.retain(|&t| now.duration_since(t) < window);

        if timestamps.len() as u32 >= max_requests {
            false
        } else {
            timestamps.push(now);
            true
        }
    }
}

pub async fn security_headers_middleware(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();

    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; font-src 'self' https://fonts.gstatic.com; connect-src 'self' ws: wss:;",
        ),
    );

    response
}
