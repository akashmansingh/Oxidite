//! Oxidite session server (oxidited) library.

#![allow(clippy::collapsible_if, clippy::useless_conversion)]

pub mod auth;
pub mod security;
pub mod tls;

use axum::{
    Json, Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode, header},
    middleware,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use futures_util::{SinkExt, StreamExt};
use oxidite_proto::{
    ClientMsg, ErrorCode, PROTOCOL_VERSION, ServerMsg, SessionId, SessionState, pack_server_msg,
    unpack_client_msg,
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

#[derive(Clone)]
pub struct AppState {
    pub manager: SessionManager,
    pub token: String,
    pub static_dir: String,
    pub known_hosts: oxidite_ssh::KnownHostsStore,
    pub security: SecurityConfig,
    pub auth: UserStore,
    pub rate_limiter: RateLimiter,
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

pub fn create_app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/api/login", post(login_handler))
        .route("/api/sessions", get(list_sessions))
        .route("/ws", get(ws_handler))
        .fallback(static_handler)
        .layer(middleware::from_fn(security_headers_middleware))
        .layer(CorsLayer::permissive())
        .with_state(state)
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

async fn list_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<SessionInfo>>, StatusCode> {
    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok());

    let user_id = if let Some(h) = auth_header {
        let token = h.strip_prefix("Bearer ").unwrap_or(h);
        if token == state.token {
            "admin".to_string()
        } else if let Some(uid) = state.auth.validate_token(token) {
            uid
        } else {
            return Err(StatusCode::UNAUTHORIZED);
        }
    } else {
        return Err(StatusCode::UNAUTHORIZED);
    };

    let all_ids = state.manager.list();
    let mut list = Vec::new();
    for id in all_ids {
        // Enforce per-user ownership unless admin
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
    let target = if path.is_empty() {
        std::path::Path::new(&state.static_dir).join("index.html")
    } else {
        std::path::Path::new(&state.static_dir).join(path)
    };

    if target.exists() && target.is_file() {
        let mime = mime_guess::from_path(&target).first_or_octet_stream();
        match tokio::fs::read(&target).await {
            Ok(content) => {
                return ([(header::CONTENT_TYPE, mime.as_ref())], content).into_response();
            }
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }

    // Fallback to index.html for SPA routes
    let index_path = std::path::Path::new(&state.static_dir).join("index.html");
    if index_path.exists() {
        if let Ok(content) = tokio::fs::read(&index_path).await {
            return Html(content).into_response();
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
    // Handshake: Expect ClientMsg::Hello first
    let user_id = match socket.recv().await {
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
                    if token == state.token {
                        "admin".to_string()
                    } else if let Some(uid) = state.auth.validate_token(&token) {
                        uid
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

    let (mut ws_sink, mut ws_stream) = socket.split();

    // Client connection loop
    let mut current_session: Option<SessionId> = None;
    let mut current_tx: Option<mpsc::Sender<ActorCmd>> = None;
    let mut pending_ssh: Option<(oxidite_proto::BackendSpec, oxidite_proto::TermSize, String)> =
        None;
    let (server_tx, mut server_rx) = mpsc::channel::<ServerMsg>(128);

    // Forward messages from server_rx to WebSocket client
    let forward_task = tokio::spawn(async move {
        while let Some(msg) = server_rx.recv().await {
            let bytes = pack_server_msg(&msg).unwrap_or_default();
            if ws_sink.send(Message::Binary(bytes.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(msg)) = ws_stream.next().await {
        let bytes = match msg {
            Message::Binary(b) => b,
            Message::Close(_) => break,
            _ => continue,
        };

        // Enforce maximum message size
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
                // Check session limits
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
                        match state
                            .manager
                            .create(oxidite_proto::BackendSpec::LocalPty { shell, cwd }, size)
                            .await
                        {
                            Ok(sid) => {
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
                // Per-user ownership check
                if user_id != "admin" && !state.auth.is_session_owner(session, &user_id) {
                    let _ = server_tx
                        .send(ServerMsg::Error {
                            code: ErrorCode::Unauthorized,
                            message: "You do not own this session".into(),
                        })
                        .await;
                    continue;
                }

                if let Some(handle) = state.manager.get(session) {
                    current_session = Some(session);
                    current_tx = Some(handle.tx.clone());

                    let (resp_tx, resp_rx) = oneshot::channel();
                    if handle
                        .tx
                        .send(ActorCmd::Attach {
                            mode,
                            last_offset,
                            resp: resp_tx,
                        })
                        .await
                        .is_ok()
                    {
                        if let Ok(mut attach_res) = resp_rx.await {
                            AuditLogger::record(AuditEvent::SessionAttached {
                                session_id: &session.to_string(),
                                mode: match mode {
                                    oxidite_proto::AttachMode::Writer => "writer",
                                    oxidite_proto::AttachMode::Viewer => "viewer",
                                },
                                user: &user_id,
                            });

                            let _ = server_tx
                                .send(ServerMsg::Attached {
                                    session,
                                    mode,
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

                            // Spawn subscriber to broadcast actor output to this client
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
            ClientMsg::Input(bytes) => {
                if let Some(tx) = &current_tx {
                    let _ = tx.send(ActorCmd::Input { bytes }).await;
                }
            }
            ClientMsg::Resize(size) => {
                if let Some(tx) = &current_tx {
                    let _ = tx.send(ActorCmd::Resize { size }).await;
                }
            }
            ClientMsg::Ack { offset } => {
                if let Some(tx) = &current_tx {
                    let _ = tx.send(ActorCmd::Ack { offset }).await;
                }
            }
            ClientMsg::Ping { nonce } => {
                let _ = server_tx.send(ServerMsg::Pong { nonce }).await;
            }
            ClientMsg::Detach => {
                if let Some(tx) = &current_tx {
                    let _ = tx.send(ActorCmd::Detach).await;
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

    // Detach if still connected
    if let Some(tx) = current_tx {
        let _ = tx.send(ActorCmd::Detach).await;
    }
    forward_task.abort();
}
