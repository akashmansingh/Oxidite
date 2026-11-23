//! Oxidite session server (oxidited) library.

#![allow(clippy::collapsible_if, clippy::useless_conversion)]

pub mod auth;
pub mod device_connections;
pub mod devices;
pub mod metrics;
pub mod routing;
pub mod security;
pub mod share;
pub mod tls;

use axum::{
    Json, Router,
    extract::{
        Path, Request, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
};
use futures_util::{SinkExt, StreamExt};
use oxidite_proto::{
    AttachMode, ClientMsg, ErrorCode, PROTOCOL_VERSION, ServerMsg, SessionId, SessionState,
    pack_server_msg, unpack_client_msg,
};
use oxidite_session::{actor::ActorCmd, manager::SessionManager};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tower_http::cors::CorsLayer;

use crate::auth::UserStore;
use crate::security::{
    AuditEvent, AuditLogger, RateLimiter, SecurityConfig, security_headers_middleware,
};
use crate::share::ShareStore;

#[derive(Clone)]
pub struct AppState {
    pub manager: SessionManager,
    pub token: String,
    pub static_dir: String,
    pub known_hosts: oxidite_ssh::KnownHostsStore,
    pub security: SecurityConfig,
    pub auth: UserStore,
    pub shares: ShareStore,
    pub rate_limiter: RateLimiter,
    pub metrics: metrics::MetricsCollector,
    pub node_id: String,
    pub directory: std::sync::Arc<dyn oxidite_directory::SessionDirectory>,
    pub devices: devices::DeviceStore,
    pub device_connections: device_connections::DeviceConnectionRegistry,
}

#[derive(Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub state: SessionState,
    pub label: Option<String>,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub user_id: String,
    pub expires_in_secs: u64,
}

#[derive(Deserialize)]
pub struct CreateShareRequest {
    pub mode: Option<String>,
    pub ttl_secs: Option<u64>,
}

#[derive(Serialize)]
pub struct CreateShareResponse {
    pub token: String,
    pub mode: String,
    pub expires_in_secs: u64,
}

pub fn create_app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/ready", get(metrics::readiness_handler))
        .route("/metrics", get(metrics::prometheus_metrics_handler))
        .route("/api/login", post(login_handler))
        .route("/api/sessions", get(list_sessions))
        .route("/api/sessions/:id/share", post(create_share_handler))
        .route("/api/shares/:token", delete(revoke_share_handler))
        .route("/api/devices/enroll", post(enroll_device_handler))
        .route("/api/devices/grant", post(issue_device_grant_handler))
        .route("/api/devices", get(list_devices_handler))
        .route("/api/devices/:id", delete(revoke_device_handler))
        .route("/api/runtime/shells", get(runtime_shells_handler))
        .route("/ws", get(ws_handler))
        .fallback(static_handler)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            route_metrics_middleware,
        ))
        .layer(middleware::from_fn(security_headers_middleware))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn route_metrics_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let start = std::time::Instant::now();
    let path = request.uri().path().to_string();
    let route_class = routing::classify_request_path(&path);
    let response = next.run(request).await;
    let duration = start.elapsed();
    let status = response.status();
    let bytes = response
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    state
        .metrics
        .observe_http(route_class, status, duration, bytes);
    response
}

async fn health_check() -> &'static str {
    "OK"
}

async fn login_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, StatusCode> {
    let client_ip = headers
        .get("x-forwarded-for")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("unknown");

    if !state.rate_limiter.check_limit(
        client_ip,
        state.security.rate_limit_per_minute,
        Duration::from_secs(60),
    ) {
        AuditLogger::record(AuditEvent::SecurityViolation {
            reason: "Rate limit exceeded on login",
            remote_ip: client_ip,
        });
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }

    let ttl = Duration::from_secs(3600);
    if let Some(token) = state.auth.login(&payload.username, &payload.password, ttl) {
        AuditLogger::record(AuditEvent::LoginAttempt {
            username: &payload.username,
            success: true,
            remote_ip: client_ip,
        });
        Ok(Json(LoginResponse {
            token: token.token,
            user_id: token.user_id,
            expires_in_secs: ttl.as_secs(),
        }))
    } else {
        AuditLogger::record(AuditEvent::LoginAttempt {
            username: &payload.username,
            success: false,
            remote_ip: client_ip,
        });
        Err(StatusCode::UNAUTHORIZED)
    }
}

async fn create_share_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<CreateShareRequest>,
) -> Result<Json<CreateShareResponse>, StatusCode> {
    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok());

    let sid = match uuid::Uuid::parse_str(&session_id) {
        Ok(s) => s,
        Err(_) => return Err(StatusCode::BAD_REQUEST),
    };

    let user_id = if let Some(h) = auth_header {
        let token = h.strip_prefix("Bearer ").unwrap_or(h).trim();
        if let Ok(principal) = state.devices.authenticate(token).await {
            principal.user_id
        } else if let Some(uid) = state.auth.validate_token(token) {
            uid
        } else if token == state.token && !token.is_empty() {
            "admin".to_string()
        } else {
            return Err(StatusCode::UNAUTHORIZED);
        }
    } else {
        return Err(StatusCode::UNAUTHORIZED);
    };

    if user_id != "admin" && !state.auth.is_session_owner(sid, &user_id) {
        return Err(StatusCode::FORBIDDEN);
    }

    let mode = match payload.mode.as_deref() {
        Some("writer") | Some("Writer") => AttachMode::Writer,
        _ => AttachMode::Viewer,
    };
    let ttl_secs = payload.ttl_secs.unwrap_or(3600);
    let share = state
        .shares
        .create_share(sid, mode, Duration::from_secs(ttl_secs));

    Ok(Json(CreateShareResponse {
        token: share.token,
        mode: match mode {
            AttachMode::Writer => "writer".into(),
            AttachMode::Viewer => "viewer".into(),
        },
        expires_in_secs: ttl_secs,
    }))
}

async fn revoke_share_handler(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> StatusCode {
    if state.shares.revoke_share(&token) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

#[derive(Deserialize)]
pub struct EnrollDeviceRequest {
    pub grant: String,
    pub label: Option<String>,
}

#[derive(Serialize)]
pub struct EnrollDeviceResponse {
    pub device_id: String,
    pub user_id: String,
    pub token: String,
    pub label: String,
}

#[derive(Deserialize)]
pub struct DeviceGrantRequest {
    pub ttl_secs: Option<u64>,
}

#[derive(Serialize)]
pub struct DeviceGrantResponse {
    pub grant: String,
    pub expires_in_secs: u64,
}

async fn enroll_device_handler(
    State(state): State<AppState>,
    Json(payload): Json<EnrollDeviceRequest>,
) -> Result<Json<EnrollDeviceResponse>, StatusCode> {
    let label = payload
        .label
        .unwrap_or_else(|| "Browser Device".to_string());
    match state.devices.enroll(&payload.grant, &label).await {
        Ok(res) => {
            let dev_id_str = res.device_id.to_string();
            AuditLogger::record(AuditEvent::DeviceEnrolled {
                device_id: &dev_id_str,
                user: &res.user_id,
            });
            Ok(Json(EnrollDeviceResponse {
                device_id: res.device_id.to_string(),
                user_id: res.user_id,
                token: res.token,
                label: res.label,
            }))
        }
        Err(_) => Err(StatusCode::UNAUTHORIZED),
    }
}

async fn issue_device_grant_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<DeviceGrantRequest>,
) -> Result<Json<DeviceGrantResponse>, StatusCode> {
    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok());

    let user_id = if let Some(h) = auth_header {
        let token = h.strip_prefix("Bearer ").unwrap_or(h).trim();
        if let Ok(principal) = state.devices.authenticate(token).await {
            principal.user_id
        } else if let Some(uid) = state.auth.validate_token(token) {
            uid
        } else if token == state.token && !token.is_empty() {
            "admin".to_string()
        } else {
            return Err(StatusCode::UNAUTHORIZED);
        }
    } else {
        return Err(StatusCode::UNAUTHORIZED);
    };

    let ttl_secs = payload.ttl_secs.unwrap_or(300);
    match state
        .devices
        .issue_enrollment_grant(&user_id, Duration::from_secs(ttl_secs))
        .await
    {
        Ok(grant) => Ok(Json(DeviceGrantResponse {
            grant,
            expires_in_secs: ttl_secs,
        })),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn list_devices_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<devices::DeviceInfo>>, StatusCode> {
    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok());

    let user_id = if let Some(h) = auth_header {
        let token = h.strip_prefix("Bearer ").unwrap_or(h).trim();
        if let Ok(principal) = state.devices.authenticate(token).await {
            principal.user_id
        } else if let Some(uid) = state.auth.validate_token(token) {
            uid
        } else if token == state.token && !token.is_empty() {
            "admin".to_string()
        } else {
            return Err(StatusCode::UNAUTHORIZED);
        }
    } else {
        return Err(StatusCode::UNAUTHORIZED);
    };

    match state.devices.list_for_user(&user_id).await {
        Ok(devices) => Ok(Json(devices)),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn revoke_device_handler(
    State(state): State<AppState>,
    Path(device_id_str): Path<String>,
    headers: HeaderMap,
) -> StatusCode {
    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok());

    let user_id = if let Some(h) = auth_header {
        let token = h.strip_prefix("Bearer ").unwrap_or(h).trim();
        if let Ok(principal) = state.devices.authenticate(token).await {
            principal.user_id
        } else if let Some(uid) = state.auth.validate_token(token) {
            uid
        } else if token == state.token && !token.is_empty() {
            "admin".to_string()
        } else {
            return StatusCode::UNAUTHORIZED;
        }
    } else {
        return StatusCode::UNAUTHORIZED;
    };

    let device_id = match uuid::Uuid::parse_str(&device_id_str) {
        Ok(id) => id,
        Err(_) => return StatusCode::BAD_REQUEST,
    };

    match state.devices.revoke(&user_id, device_id).await {
        Ok(()) => {
            state.device_connections.close_local(device_id);
            StatusCode::NO_CONTENT
        }
        Err(devices::DeviceAuthError::NotFound) => StatusCode::NOT_FOUND,
        Err(devices::DeviceAuthError::Unauthorized) => StatusCode::UNAUTHORIZED,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn runtime_shells_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<oxidite_proto::ShellCapability>>, StatusCode> {
    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok());

    let token = match auth_header.and_then(|h| h.strip_prefix("Bearer ")) {
        Some(t) => t.trim(),
        None => return Err(StatusCode::UNAUTHORIZED),
    };

    let _user_id = if let Ok(principal) = state.devices.authenticate(token).await {
        principal.user_id
    } else if let Some(uid) = state.auth.validate_token(token) {
        uid
    } else if token == state.token && !token.is_empty() {
        "admin".to_string()
    } else {
        return Err(StatusCode::UNAUTHORIZED);
    };

    let pwsh_available = oxidite_pty::is_powershell_available();
    let capabilities = vec![
        oxidite_proto::ShellCapability {
            id: "default".to_string(),
            available: true,
        },
        oxidite_proto::ShellCapability {
            id: "SystemPowerShell".to_string(),
            available: pwsh_available,
        },
    ];

    Ok(Json(capabilities))
}

async fn list_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<SessionInfo>>, StatusCode> {
    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok());

    let user_id = if let Some(h) = auth_header {
        let token = h.strip_prefix("Bearer ").unwrap_or(h).trim();
        if let Ok(principal) = state.devices.authenticate(token).await {
            principal.user_id
        } else if let Some(uid) = state.auth.validate_token(token) {
            uid
        } else if token == state.token && !token.is_empty() {
            "admin".to_string()
        } else {
            return Err(StatusCode::UNAUTHORIZED);
        }
    } else {
        return Err(StatusCode::UNAUTHORIZED);
    };

    let all_ids = state.manager.list();
    let mut list = Vec::new();
    for id in all_ids {
        if user_id != "admin" && !state.auth.is_session_owner(id, &user_id) {
            continue;
        }

        if let Some(handle) = state.manager.get(id) {
            let (tx, rx) = oneshot::channel();
            if handle
                .tx
                .send(ActorCmd::GetState { resp: tx })
                .await
                .is_ok()
            {
                if let Ok(sess_state) = rx.await {
                    list.push(SessionInfo {
                        id: id.to_string(),
                        state: sess_state,
                        label: Some(format!("session-{}", &id.to_string()[..8])),
                    });
                }
            }
        }
    }
    Ok(Json(list))
}

async fn static_handler(State(state): State<AppState>, uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let static_dir = std::path::Path::new(&state.static_dir);
    let target = if path.is_empty() {
        static_dir.join("index.html")
    } else {
        match routing::resolve_static_path(static_dir, path) {
            Some(target) => target,
            None => return StatusCode::BAD_REQUEST.into_response(),
        }
    };

    if target.exists() && target.is_file() {
        let mime = mime_guess::from_path(&target).first_or_octet_stream();
        match tokio::fs::read(&target).await {
            Ok(content) => {
                return (
                    [
                        (header::CONTENT_TYPE, mime.as_ref()),
                        (
                            header::CACHE_CONTROL,
                            routing::cache_control(routing::classify_request_path(uri.path())),
                        ),
                    ],
                    content,
                )
                    .into_response();
            }
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }

    let index_path = static_dir.join("index.html");
    if index_path.exists() {
        if let Ok(content) = tokio::fs::read(&index_path).await {
            return (
                [(
                    header::CACHE_CONTROL,
                    routing::cache_control(routing::RouteClass::Html),
                )],
                Html(content),
            )
                .into_response();
        }
    }

    StatusCode::NOT_FOUND.into_response()
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    let origin = headers.get(header::ORIGIN).and_then(|h| h.to_str().ok());
    if !state.security.is_origin_allowed(origin) {
        AuditLogger::record(AuditEvent::SecurityViolation {
            reason: "Rejected WebSocket upgrade: unauthorized origin",
            remote_ip: origin.unwrap_or("unknown"),
        });
        return StatusCode::FORBIDDEN.into_response();
    }

    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: AppState) {
    let ws_start = std::time::Instant::now();
    // Handshake: Expect ClientMsg::Hello first
    let (user_id, shared_access, device_id) = match socket.recv().await {
        Some(Ok(Message::Binary(bin))) => {
            match unpack_client_msg(&bin) {
                Ok(ClientMsg::Hello {
                    protocol_version,
                    token,
                }) => {
                    if protocol_version != PROTOCOL_VERSION {
                        let err = pack_server_msg(&ServerMsg::Error {
                            code: ErrorCode::ProtocolMismatch,
                            message: format!(
                                "Version mismatch: expected {PROTOCOL_VERSION}, got {protocol_version}"
                            ),
                        })
                        .unwrap_or_default();
                        let _ = socket.send(Message::Binary(err.into())).await;
                        let _ = socket.close().await;
                        return;
                    }

                    // Authenticate token
                    if let Ok(principal) = state.devices.authenticate(&token).await {
                        let dev_id_str = principal.device_id.to_string();
                        AuditLogger::record(AuditEvent::DeviceAuthenticated {
                            device_id: &dev_id_str,
                            user: &principal.user_id,
                        });
                        (principal.user_id, None, Some(principal.device_id))
                    } else if token == state.token && !token.is_empty() {
                        ("admin".to_string(), None, None)
                    } else if token.starts_with("share_") {
                        if let Some((sid, mode)) = state.shares.validate_share(&token) {
                            (format!("viewer_{}", &token[6..14]), Some((sid, mode)), None)
                        } else {
                            let err = pack_server_msg(&ServerMsg::Error {
                                code: ErrorCode::Unauthorized,
                                message: "Share link expired or revoked".into(),
                            })
                            .unwrap_or_default();
                            let _ = socket.send(Message::Binary(err.into())).await;
                            let _ = socket.close().await;
                            return;
                        }
                    } else if let Some(uid) = state.auth.validate_token(&token) {
                        (uid, None, None)
                    } else {
                        let err = pack_server_msg(&ServerMsg::Error {
                            code: ErrorCode::Unauthorized,
                            message: "Invalid access token".into(),
                        })
                        .unwrap_or_default();
                        let _ = socket.send(Message::Binary(err.into())).await;
                        let _ = socket.close().await;
                        return;
                    }
                }
                _ => {
                    let _ = socket.close().await;
                    return;
                }
            }
        }
        _ => {
            let _ = socket.close().await;
            return;
        }
    };

    // Send Welcome
    let welcome = pack_server_msg(&ServerMsg::Welcome {
        server_version: env!("CARGO_PKG_VERSION").to_string(),
    })
    .unwrap_or_default();
    if socket.send(Message::Binary(welcome.into())).await.is_err() {
        return;
    }
    state
        .metrics
        .observe_websocket_handshake(ws_start.elapsed());

    let (mut ws_sink, mut ws_stream) = socket.split();

    // Client connection state
    let client_id = rand::random::<u64>();
    let (close_tx, mut close_rx) = mpsc::channel::<()>(1);
    if let Some(dev_id) = device_id {
        if !state
            .device_connections
            .register(dev_id, client_id, close_tx)
        {
            let _ = ws_sink.close().await;
            return;
        }
    }

    let mut current_session: Option<SessionId> = None;
    let mut current_tx: Option<mpsc::Sender<ActorCmd>> = None;
    let mut pending_ssh: Option<(oxidite_proto::BackendSpec, oxidite_proto::TermSize, String)> =
        None;
    let (server_tx, mut server_rx) = mpsc::channel::<ServerMsg>(256);

    // Forward messages from server_rx to WebSocket client
    let forward_task = tokio::spawn(async move {
        while let Some(msg) = server_rx.recv().await {
            let bytes = pack_server_msg(&msg).unwrap_or_default();
            if ws_sink.send(Message::Binary(bytes.into())).await.is_err() {
                break;
            }
        }
        let _ = ws_sink.close().await;
    });

    let mut revalidate_interval = tokio::time::interval(Duration::from_secs(5));
    revalidate_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        let msg = tokio::select! {
            _ = close_rx.recv() => {
                break;
            }
            _ = revalidate_interval.tick() => {
                if let Some(dev_id) = device_id {
                    if state.device_connections.requires_revalidation(client_id, Duration::from_secs(30)) {
                        match state.devices.is_revoked(dev_id).await {
                            Ok(true) | Err(_) => {
                                state.device_connections.close_local(dev_id);
                                break;
                            }
                            Ok(false) => {
                                state.device_connections.mark_validated(client_id);
                            }
                        }
                    }
                }
                continue;
            }
            res = ws_stream.next() => {
                match res {
                    Some(Ok(m)) => m,
                    _ => break,
                }
            }
        };

        let bytes = match msg {
            Message::Binary(b) => b,
            Message::Close(_) => break,
            _ => continue,
        };

        if bytes.len() > state.security.max_message_size {
            let _ = server_tx
                .send(ServerMsg::Error {
                    code: ErrorCode::InvalidMessage,
                    message: "Message exceeds maximum allowed size".into(),
                })
                .await;
            continue;
        }

        let client_msg = match unpack_client_msg(&bytes) {
            Ok(m) => m,
            Err(e) => {
                let _ = server_tx
                    .send(ServerMsg::Error {
                        code: ErrorCode::InvalidMessage,
                        message: e.to_string(),
                    })
                    .await;
                continue;
            }
        };

        match client_msg {
            ClientMsg::Create { backend, size } => {
                if shared_access.is_some() {
                    let _ = server_tx
                        .send(ServerMsg::Error {
                            code: ErrorCode::Unauthorized,
                            message: "Share tokens cannot create sessions".into(),
                        })
                        .await;
                    continue;
                }

                let current_user_sessions = state.auth.user_sessions(&user_id);
                if user_id != "admin"
                    && current_user_sessions.len() >= state.security.max_sessions_per_user
                {
                    let _ = server_tx
                        .send(ServerMsg::Error {
                            code: ErrorCode::BackendError,
                            message: format!(
                                "Maximum session limit ({}) reached",
                                state.security.max_sessions_per_user
                            ),
                        })
                        .await;
                    continue;
                }

                match backend {
                    oxidite_proto::BackendSpec::LocalPty { shell, cwd } => {
                        if !state.security.is_local_pty_allowed() {
                            let _ = server_tx
                                .send(ServerMsg::Error {
                                    code: ErrorCode::Unauthorized,
                                    message: "Local PTY is disabled on non-loopback binds".into(),
                                })
                                .await;
                            continue;
                        }

                        if let Err(e) = oxidite_pty::resolve_safe_shell(shell.as_deref()) {
                            let code = match e {
                                oxidite_pty::ShellError::ArbitraryPathRejected(_) => {
                                    ErrorCode::InvalidMessage
                                }
                                oxidite_pty::ShellError::PowerShellUnavailable => {
                                    ErrorCode::BackendError
                                }
                            };
                            let _ = server_tx
                                .send(ServerMsg::Error {
                                    code,
                                    message: e.to_string(),
                                })
                                .await;
                            continue;
                        }

                        match state
                            .manager
                            .create(oxidite_proto::BackendSpec::LocalPty { shell, cwd }, size)
                            .await
                        {
                            Ok(sid) => {
                                let _ = state
                                    .directory
                                    .claim(sid, state.node_id.clone(), Duration::from_secs(60))
                                    .await;
                                state.auth.set_session_owner(sid, &user_id);
                                AuditLogger::record(AuditEvent::SessionCreated {
                                    session_id: &sid.to_string(),
                                    backend_kind: "local_pty",
                                    user: &user_id,
                                });
                                current_session = Some(sid);
                                let _ = server_tx.send(ServerMsg::Created { session: sid }).await;
                            }
                            Err(e) => {
                                let _ = server_tx
                                    .send(ServerMsg::Error {
                                        code: ErrorCode::BackendError,
                                        message: e.to_string(),
                                    })
                                    .await;
                            }
                        }
                    }
                    oxidite_proto::BackendSpec::Ssh {
                        host,
                        port,
                        user,
                        auth,
                        multiplexer,
                    } => {
                        let fp = format!("SHA256:{:x}", host.len() * 31 + port as usize);
                        let status = state.known_hosts.check(&host, &fp);
                        match status {
                            oxidite_proto::HostKeyStatus::Unknown => {
                                pending_ssh = Some((
                                    oxidite_proto::BackendSpec::Ssh {
                                        host: host.clone(),
                                        port,
                                        user,
                                        auth,
                                        multiplexer,
                                    },
                                    size,
                                    fp.clone(),
                                ));
                                let _ = server_tx
                                    .send(ServerMsg::HostKeyPrompt {
                                        host,
                                        fingerprint: fp,
                                        known: oxidite_proto::HostKeyStatus::Unknown,
                                    })
                                    .await;
                            }
                            oxidite_proto::HostKeyStatus::Mismatch => {
                                let _ = server_tx
                                    .send(ServerMsg::Error {
                                        code: ErrorCode::BackendError,
                                        message: format!(
                                            "Host key mismatch for {host}! Connection refused."
                                        ),
                                    })
                                    .await;
                            }
                            oxidite_proto::HostKeyStatus::Trusted => {
                                let ssh_cfg = oxidite_ssh::SshConfig {
                                    host,
                                    port,
                                    user,
                                    auth,
                                    multiplexer,
                                    keepalive_interval_secs: 15,
                                    keepalive_max_misses: 3,
                                };
                                let (backend, _, _) = oxidite_ssh::SshBackend::new(ssh_cfg, size);
                                let sid = oxidite_proto::SessionId::new_v4();
                                state
                                    .manager
                                    .create_with_backend(sid, size, Box::new(backend));
                                let _ = state
                                    .directory
                                    .claim(sid, state.node_id.clone(), Duration::from_secs(60))
                                    .await;
                                state.auth.set_session_owner(sid, &user_id);
                                AuditLogger::record(AuditEvent::SessionCreated {
                                    session_id: &sid.to_string(),
                                    backend_kind: "ssh",
                                    user: &user_id,
                                });
                                current_session = Some(sid);
                                let _ = server_tx.send(ServerMsg::Created { session: sid }).await;
                            }
                        }
                    }
                }
            }
            ClientMsg::HostKeyDecision { accept } => {
                if let Some((spec, size, fp)) = pending_ssh.take() {
                    if accept {
                        if let oxidite_proto::BackendSpec::Ssh {
                            host,
                            port,
                            user,
                            auth,
                            multiplexer,
                        } = spec
                        {
                            state.known_hosts.trust(host.clone(), fp);
                            let ssh_cfg = oxidite_ssh::SshConfig {
                                host,
                                port,
                                user,
                                auth,
                                multiplexer,
                                keepalive_interval_secs: 15,
                                keepalive_max_misses: 3,
                            };
                            let (backend, _, _) = oxidite_ssh::SshBackend::new(ssh_cfg, size);
                            let sid = oxidite_proto::SessionId::new_v4();
                            state
                                .manager
                                .create_with_backend(sid, size, Box::new(backend));
                            state.auth.set_session_owner(sid, &user_id);
                            AuditLogger::record(AuditEvent::SessionCreated {
                                session_id: &sid.to_string(),
                                backend_kind: "ssh",
                                user: &user_id,
                            });
                            current_session = Some(sid);
                            let _ = server_tx.send(ServerMsg::Created { session: sid }).await;
                        }
                    } else {
                        let _ = server_tx
                            .send(ServerMsg::Error {
                                code: ErrorCode::Unauthorized,
                                message: "Host key was not accepted by user".into(),
                            })
                            .await;
                    }
                }
            }
            ClientMsg::Attach {
                session,
                last_offset,
                mode,
            } => {
                let actual_mode = if let Some((shared_sid, share_mode)) = shared_access {
                    if session != shared_sid {
                        let _ = server_tx
                            .send(ServerMsg::Error {
                                code: ErrorCode::Unauthorized,
                                message: "Share token not valid for this session".into(),
                            })
                            .await;
                        continue;
                    }
                    share_mode
                } else {
                    if user_id != "admin" && !state.auth.is_session_owner(session, &user_id) {
                        let _ = server_tx
                            .send(ServerMsg::Error {
                                code: ErrorCode::Unauthorized,
                                message: "You do not own this session".into(),
                            })
                            .await;
                        continue;
                    }
                    mode
                };

                // Multi-node ownership lookup check
                if let Ok(Some(owner)) = state.directory.lookup(session).await {
                    if owner.node != state.node_id {
                        let _ = server_tx
                            .send(ServerMsg::Error {
                                code: ErrorCode::SessionNotFound,
                                message: format!(
                                    "Session {session} is hosted on remote node {}",
                                    owner.node
                                ),
                            })
                            .await;
                        continue;
                    }
                }

                if let Some(handle) = state.manager.get(session) {
                    current_session = Some(session);
                    current_tx = Some(handle.tx.clone());

                    let (resp_tx, resp_rx) = oneshot::channel();
                    if handle
                        .tx
                        .send(ActorCmd::Attach {
                            mode: actual_mode,
                            last_offset,
                            client_id: Some(client_id),
                            resp: resp_tx,
                        })
                        .await
                        .is_ok()
                    {
                        if let Ok(mut attach_res) = resp_rx.await {
                            AuditLogger::record(AuditEvent::SessionAttached {
                                session_id: &session.to_string(),
                                mode: match actual_mode {
                                    oxidite_proto::AttachMode::Writer => "writer",
                                    oxidite_proto::AttachMode::Viewer => "viewer",
                                },
                                user: &user_id,
                            });

                            let _ = server_tx
                                .send(ServerMsg::Attached {
                                    session,
                                    mode: actual_mode,
                                    start: attach_res.start.clone(),
                                })
                                .await;

                            if let Some(snap) = attach_res.snapshot {
                                let _ = server_tx
                                    .send(ServerMsg::Snapshot {
                                        offset: attach_res.current_offset,
                                        bytes: snap,
                                    })
                                    .await;
                            } else {
                                for chunk in attach_res.replay_chunks {
                                    let _ = server_tx
                                        .send(ServerMsg::Output {
                                            offset: last_offset.unwrap_or(0),
                                            bytes: chunk,
                                        })
                                        .await;
                                }
                            }

                            let stx = server_tx.clone();
                            tokio::spawn(async move {
                                while let Ok(smsg) = attach_res.rx.recv().await {
                                    if stx.send(smsg).await.is_err() {
                                        break;
                                    }
                                }
                            });
                        }
                    }
                } else {
                    let _ = server_tx
                        .send(ServerMsg::Error {
                            code: ErrorCode::SessionNotFound,
                            message: format!("Session {session} not found"),
                        })
                        .await;
                }
            }
            ClientMsg::TakeoverWriter => {
                if let Some(tx) = &current_tx {
                    let (resp_tx, resp_rx) = oneshot::channel();
                    let _ = tx
                        .send(ActorCmd::TakeoverWriter {
                            client_id,
                            resp: resp_tx,
                        })
                        .await;
                    let _ = resp_rx.await;
                }
            }
            ClientMsg::Input(bytes) => {
                if let Some(tx) = &current_tx {
                    let _ = tx
                        .send(ActorCmd::Input {
                            client_id: Some(client_id),
                            bytes,
                        })
                        .await;
                }
            }
            ClientMsg::Resize(size) => {
                if let Some(tx) = &current_tx {
                    let _ = tx.send(ActorCmd::Resize { size }).await;
                }
            }
            ClientMsg::Ack { offset } => {
                if let Some(tx) = &current_tx {
                    let _ = tx
                        .send(ActorCmd::Ack {
                            client_id: Some(client_id),
                            offset,
                        })
                        .await;
                }
            }
            ClientMsg::Ping { nonce } => {
                let _ = server_tx.send(ServerMsg::Pong { nonce }).await;
            }
            ClientMsg::Detach => {
                if let Some(tx) = &current_tx {
                    let _ = tx
                        .send(ActorCmd::Detach {
                            client_id: Some(client_id),
                        })
                        .await;
                }
                break;
            }
            ClientMsg::Kill => {
                if let Some(sid) = current_session {
                    if user_id != "admin" && !state.auth.is_session_owner(sid, &user_id) {
                        let _ = server_tx
                            .send(ServerMsg::Error {
                                code: ErrorCode::Unauthorized,
                                message: "You do not own this session".into(),
                            })
                            .await;
                        continue;
                    }
                    AuditLogger::record(AuditEvent::SessionTerminated {
                        session_id: &sid.to_string(),
                        user: &user_id,
                    });
                    state.auth.remove_session_owner(sid);
                }
                if let Some(tx) = &current_tx {
                    let _ = tx.send(ActorCmd::Kill).await;
                }
                break;
            }
            _ => {}
        }
    }

    state.device_connections.unregister(client_id);
    if let Some(tx) = current_tx {
        let _ = tx
            .send(ActorCmd::Detach {
                client_id: Some(client_id),
            })
            .await;
    }
    forward_task.abort();
}
