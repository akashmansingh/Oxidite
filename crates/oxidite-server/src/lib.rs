//! Oxidite session server (oxidited) library.

#![allow(clippy::collapsible_if, clippy::useless_conversion)]

use axum::{
    Json, Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use oxidite_proto::{
    ClientMsg, ErrorCode, PROTOCOL_VERSION, ServerMsg, SessionId, SessionState, pack_server_msg,
    unpack_client_msg,
};
use oxidite_session::{actor::ActorCmd, manager::SessionManager};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};
use tower_http::cors::CorsLayer;

#[derive(Clone)]
pub struct AppState {
    pub manager: SessionManager,
    pub token: String,
    pub static_dir: String,
}

#[derive(Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub state: SessionState,
    pub label: Option<String>,
}

pub fn create_app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/api/sessions", get(list_sessions))
        .route("/ws", get(ws_handler))
        .fallback(static_handler)
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn health_check() -> &'static str {
    "OK"
}

async fn list_sessions(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Vec<SessionInfo>>, StatusCode> {
    // Check Authorization header: Bearer <token>
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok());
    let expected = format!("Bearer {}", state.token);
    if auth != Some(&expected) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let ids = state.manager.list();
    let mut list = Vec::new();
    for id in ids {
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
                return ([(axum::http::header::CONTENT_TYPE, mime.as_ref())], content)
                    .into_response();
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

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: AppState) {
    // Handshake: Expect ClientMsg::Hello first
    let hello = match socket.recv().await {
        Some(Ok(Message::Binary(bin))) => {
            match unpack_client_msg(&bin) {
                Ok(ClientMsg::Hello {
                    protocol_version,
                    token,
                }) => {
                    if protocol_version != PROTOCOL_VERSION {
                        let err = pack_server_msg(&ServerMsg::Error {
                        code: ErrorCode::ProtocolMismatch,
                        message: format!("Version mismatch: expected {PROTOCOL_VERSION}, got {protocol_version}"),
                    }).unwrap_or_default();
                        let _ = socket.send(Message::Binary(err.into())).await;
                        let _ = socket.close().await;
                        return;
                    }
                    if token != state.token {
                        let err = pack_server_msg(&ServerMsg::Error {
                            code: ErrorCode::Unauthorized,
                            message: "Invalid access token".into(),
                        })
                        .unwrap_or_default();
                        let _ = socket.send(Message::Binary(err.into())).await;
                        let _ = socket.close().await;
                        return;
                    }
                    true
                }
                _ => false,
            }
        }
        _ => false,
    };

    if !hello {
        let _ = socket.close().await;
        return;
    }

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
    let mut _current_session: Option<SessionId> = None;
    let mut current_tx: Option<mpsc::Sender<ActorCmd>> = None;
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
                match state.manager.create(backend, size).await {
                    Ok(sid) => {
                        _current_session = Some(sid);
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
            ClientMsg::Attach {
                session,
                last_offset,
                mode,
            } => {
                if let Some(handle) = state.manager.get(session) {
                    _current_session = Some(session);
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
