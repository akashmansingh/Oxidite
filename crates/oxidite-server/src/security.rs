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
    pub bind_addr: Option<std::net::SocketAddr>,
    pub workspace_roots: Vec<std::path::PathBuf>,
    pub default_cwd: Option<std::path::PathBuf>,
    pub ssh_key_dir: Option<std::path::PathBuf>,
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
            bind_addr: None,
            workspace_roots: Vec::new(),
            default_cwd: None,
            ssh_key_dir: None,
        }
    }
}

impl SecurityConfig {
    /// Returns whether LocalPty is allowed under this configuration and bind address.
    #[must_use]
    pub fn is_local_pty_allowed(&self) -> bool {
        if self.allow_non_loopback_pty {
            return true;
        }
        match self.bind_addr {
            Some(addr) => addr.ip().is_loopback(),
            None => true,
        }
    }
    #[must_use]
    pub fn is_origin_allowed(&self, origin: Option<&str>) -> bool {
        self.is_origin_allowed_for_host(origin, None)
    }

    #[must_use]
    pub fn is_origin_allowed_for_host(&self, origin: Option<&str>, host: Option<&str>) -> bool {
        match origin {
            None => true, // Same-origin or non-browser client (curl, test runner)
            Some(o) => {
                if self.allowed_origins.iter().any(|allowed| allowed == o) {
                    return true;
                }
                if Self::is_loopback_origin(o) {
                    return true;
                }
                if let Some(h) = host.map(str::trim).filter(|h| !h.is_empty()) {
                    if let Some(origin_authority) = Self::origin_authority(o) {
                        return origin_authority.eq_ignore_ascii_case(h);
                    }
                }
                false
            }
        }
    }

    fn origin_authority(origin: &str) -> Option<&str> {
        let rest = origin
            .strip_prefix("http://")
            .or_else(|| origin.strip_prefix("https://"))?;
        if rest.is_empty()
            || rest.contains('/')
            || rest.contains('?')
            || rest.contains('#')
            || rest.contains('@')
        {
            return None;
        }
        Some(rest)
    }

    fn host_port_matches(authority: &str, name: &str) -> bool {
        authority.eq_ignore_ascii_case(name)
            || authority
                .strip_prefix(name)
                .and_then(|rest| rest.strip_prefix(':'))
                .is_some_and(|port| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
    }

    fn is_loopback_origin(origin: &str) -> bool {
        let Some(authority) = Self::origin_authority(origin) else {
            return false;
        };
        Self::host_port_matches(authority, "localhost")
            || Self::host_port_matches(authority, "127.0.0.1")
            || Self::host_port_matches(authority, "[::1]")
    }

    #[must_use]
    pub fn is_non_loopback(&self) -> bool {
        matches!(self.bind_addr, Some(addr) if !addr.ip().is_loopback())
    }

    /// Resolves a neutral initial directory for local shells so sessions do not land
    /// in the gateway binary/source directory by default.
    #[must_use]
    pub fn resolve_default_cwd(&self) -> Option<std::path::PathBuf> {
        if let Some(ref custom) = self.default_cwd {
            if let Some(canon) = canonical_clean_dir(custom)
                && self.is_workspace_path_allowed(&canon)
            {
                return Some(canon);
            }
        }
        if let Some(first) = self.workspace_roots.first()
            && let Some(canon) = canonical_clean_dir(first)
        {
            return Some(canon);
        }
        user_home_dir()
            .and_then(|p| canonical_clean_dir(&p))
            .filter(|p| self.is_workspace_path_allowed(p))
    }

    /// Checks whether a canonical filesystem path is allowed as a session workspace directory.
    #[must_use]
    pub fn is_workspace_path_allowed(&self, canonical_path: &std::path::Path) -> bool {
        let candidate = canonical_clean_path(canonical_path)
            .unwrap_or_else(|| strip_win_verbatim_prefix(canonical_path.to_path_buf()));
        if is_sensitive_system_path(&candidate) {
            return false;
        }
        if self.workspace_roots.is_empty() {
            return true;
        }
        self.workspace_roots.iter().any(|root| {
            canonical_clean_path(root)
                .map(|canon_root| candidate.starts_with(&canon_root))
                .unwrap_or(false)
        })
    }

    /// Checks whether a resolved Git repository root is safe to inspect and operate on.
    #[must_use]
    pub fn is_git_root_allowed(&self, repo_root: &std::path::Path) -> bool {
        let canon_root = canonical_clean_path(repo_root)
            .unwrap_or_else(|| strip_win_verbatim_prefix(repo_root.to_path_buf()));
        if canon_root.parent().is_none() || is_sensitive_system_path(&canon_root) {
            return false;
        }
        if let Some(home) = user_home_dir().and_then(|h| canonical_clean_path(&h))
            && canon_root == home
        {
            return false;
        }
        if self.workspace_roots.is_empty() {
            return true;
        }
        self.workspace_roots.iter().any(|root| {
            canonical_clean_path(root)
                .map(|allowed| canon_root.starts_with(&allowed))
                .unwrap_or(false)
        })
    }

    /// Validates whether a server-side SSH private key path may be read for SSH auth.
    pub fn validate_ssh_key_path(&self, raw_path: &str) -> Result<(), &'static str> {
        let trimmed = raw_path.trim();
        if trimmed.is_empty() || trimmed.contains('\0') {
            return Err("SSH private key path is required");
        }
        if self.is_non_loopback() && self.ssh_key_dir.is_none() {
            return Err(
                "Server-side SSH key files require OXIDITE_SSH_KEY_DIR on non-loopback binds",
            );
        }
        let expanded = expand_tilde_path(trimmed);
        let canon_key = std::fs::canonicalize(&expanded)
            .map_err(|_| "SSH key path is not permitted or unavailable")?;
        if !canon_key.is_file() {
            return Err("SSH key path is not permitted or unavailable");
        }
        if let Some(ref allowed_dir) = self.ssh_key_dir {
            let canon_dir = std::fs::canonicalize(allowed_dir)
                .map_err(|_| "SSH key path is not permitted or unavailable")?;
            if !canon_key.starts_with(&canon_dir) {
                return Err("SSH key path is not permitted or unavailable");
            }
        }
        Ok(())
    }
}

fn strip_win_verbatim_prefix(path: std::path::PathBuf) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        let s = path.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            return std::path::PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return std::path::PathBuf::from(rest);
        }
    }
    path
}

#[must_use]
pub fn canonical_clean_path(path: &std::path::Path) -> Option<std::path::PathBuf> {
    std::fs::canonicalize(path)
        .ok()
        .map(strip_win_verbatim_prefix)
}

fn canonical_clean_dir(path: &std::path::Path) -> Option<std::path::PathBuf> {
    canonical_clean_path(path).filter(|p| p.is_dir())
}

fn user_home_dir() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(std::path::PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(std::path::PathBuf::from)
    }
}

fn expand_tilde_path(raw: &str) -> std::path::PathBuf {
    if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\"))
        && let Some(home) = user_home_dir()
    {
        return home.join(rest);
    }
    std::path::PathBuf::from(raw)
}

fn normalize_path_str(p: &std::path::Path) -> String {
    let raw = p.to_string_lossy().replace('\\', "/");
    raw.strip_prefix("//?/")
        .unwrap_or(&raw)
        .to_ascii_lowercase()
}

fn is_sensitive_system_path(path: &std::path::Path) -> bool {
    if path.parent().is_none() {
        return true;
    }
    let norm = normalize_path_str(path);
    #[cfg(windows)]
    {
        let sys_root = std::env::var("SystemRoot")
            .unwrap_or_else(|_| "C:/Windows".to_string())
            .replace('\\', "/")
            .to_ascii_lowercase();
        let prog_data = std::env::var("ProgramData")
            .unwrap_or_else(|_| "C:/ProgramData".to_string())
            .replace('\\', "/")
            .to_ascii_lowercase();
        let blocked = [
            sys_root.as_str(),
            prog_data.as_str(),
            "c:/windows",
            "c:/programdata",
        ];
        if blocked
            .iter()
            .any(|b| norm == *b || norm.starts_with(&format!("{b}/")))
        {
            return true;
        }
    }
    #[cfg(not(windows))]
    {
        let blocked = [
            "/etc", "/proc", "/sys", "/dev", "/boot", "/sbin", "/usr", "/var",
        ];
        if blocked
            .iter()
            .any(|b| norm == *b || norm.starts_with(&format!("{b}/")))
        {
            return true;
        }
    }
    false
}

/// Extracts a bounded client rate-limit bucket key without retaining unbounded header strings.
#[must_use]
pub fn rate_limit_key(headers: &axum::http::HeaderMap, scope: &str) -> String {
    let ip = headers
        .get("x-forwarded-for")
        .or_else(|| headers.get("x-real-ip"))
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 64)
        .unwrap_or("direct");
    format!("{scope}:{ip}")
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
        if map.len() >= 4096 {
            map.retain(|_, ts| {
                ts.retain(|&t| now.duration_since(t) < window);
                !ts.is_empty()
            });
            if map.len() >= 4096 {
                return false;
            }
        }
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

pub async fn security_headers_middleware(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    request: Request,
    next: Next,
) -> Response {
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
        header::HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=(), usb=()"),
    );
    if state.security.is_non_loopback() {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        );
    }
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; font-src 'self' https://fonts.gstatic.com; img-src 'self'; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'; connect-src 'self' ws: wss:;",
        ),
    );

    response
}
