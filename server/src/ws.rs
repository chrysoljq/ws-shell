use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};
use ws_shell_common::{
    ClientMsg, ExecRequest, ExecResponse, LoginRequest, LoginResponse, MessageResponse, ServerMsg,
};

use crate::auth::{self, ApiErrorResponse, AuthUser};
use crate::state::AppState;

pub fn build_router(state: Arc<AppState>) -> Router {
    let static_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("static");

    Router::new()
        // ── Public ──────────────────────────────────────────
        .route("/", get(serve_index))
        .route("/api/v1/login", post(login))
        .route("/api/v1/agent/latest", get(download_client))
        .route("/api/v1/log", post(upload_log))
        // ── Protected (AuthUser extractor) ──────────────────
        .route("/api/v1/clients", get(list_clients))
        .route(
            "/api/v1/clients/{client_id}",
            get(get_client).delete(disconnect_client),
        )
        .route("/api/v1/clients/{client_id}/exec", post(exec_command))
        .route(
            "/api/v1/clients/{client_id}/kill/{task_id}",
            post(kill_task),
        )
        .route("/api/v1/tasks/{task_id}", get(get_task))
        // ── WebSocket ───────────────────────────────────────
        .route("/ws", get(ws_handler))
        .route("/ws/client", get(ws_client_handler))
        .nest_service("/static", tower_http::services::ServeDir::new(static_dir))
        .with_state(state)
}

// ── WebUI ────────────────────────────────────────────────────────

async fn serve_index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

async fn download_client() -> Response {
    let path = std::env::current_dir()
        .unwrap_or_default()
        .join("agent-linux-amd64.gz");
    if !path.exists() {
        return ApiErrorResponse::not_found("agent binary not found").into_response();
    }
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            [
                ("Content-Type", "application/gzip"),
                (
                    "Content-Disposition",
                    "attachment; filename=\"agent-linux-amd64.gz\"",
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => {
            ApiErrorResponse::internal(format!("failed to read agent: {}", e)).into_response()
        }
    }
}

// ── Log upload (public, used by agents) ──────────────────────────

#[derive(serde::Deserialize)]
struct LogUpload {
    hostname: Option<String>,
    level: Option<String>,
    message: String,
}

async fn upload_log(
    State(state): State<Arc<AppState>>,
    Json(log): Json<LogUpload>,
) -> Json<MessageResponse> {
    let hostname = log.hostname.unwrap_or_else(|| "unknown".to_string());
    let level = log.level.unwrap_or_else(|| "info".to_string());
    let msg = format!("[{}] [{}] {}", hostname, level, log.message);

    match level.as_str() {
        "error" | "warn" => warn!("{}", msg),
        _ => info!("{}", msg),
    }

    let webui_msg = serde_json::json!({
        "type": "Log",
        "hostname": hostname,
        "level": level,
        "message": log.message,
    });
    state.broadcast_to_webui(&webui_msg.to_string());

    Json(MessageResponse {
        message: "ok".to_string(),
    })
}

// ── Auth ─────────────────────────────────────────────────────────

async fn login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiErrorResponse> {
    if req.username != state.admin_username
        || !auth::verify_password(&state.admin_password_hash, &req.password)
    {
        return Err(ApiErrorResponse::unauthorized("invalid credentials"));
    }
    let token = auth::create_token(&state.jwt_secret, &req.username);
    Ok(Json(LoginResponse { token }))
}

// ── REST API (all protected by AuthUser extractor) ───────────────

async fn list_clients(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
) -> Json<Vec<ws_shell_common::ClientInfo>> {
    Json(state.list_clients())
}

async fn get_client(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(client_id): axum::extract::Path<String>,
) -> Result<Json<ws_shell_common::ClientInfo>, ApiErrorResponse> {
    state
        .get_client(&client_id)
        .map(Json)
        .ok_or_else(|| ApiErrorResponse::not_found(format!("client {} not found", client_id)))
}

async fn disconnect_client(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(client_id): axum::extract::Path<String>,
) -> Result<Json<MessageResponse>, ApiErrorResponse> {
    if state.disconnect_client(&client_id) {
        info!("Client {} disconnected via API", client_id);
        Ok(Json(MessageResponse {
            message: format!("client {} disconnected", client_id),
        }))
    } else {
        Err(ApiErrorResponse::not_found(format!(
            "client {} not found",
            client_id
        )))
    }
}

async fn exec_command(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(client_id): axum::extract::Path<String>,
    Json(req): Json<ExecRequest>,
) -> Result<Json<ExecResponse>, ApiErrorResponse> {
    let id = uuid::Uuid::new_v4().to_string();
    let msg = ServerMsg::Exec {
        id: id.clone(),
        cmd: req.cmd,
        cwd: req.cwd,
        env: req.env,
    };
    state
        .send_to_client(&client_id, &msg)
        .map_err(ApiErrorResponse::not_found)?;
    state.task_create(&id);
    Ok(Json(ExecResponse { id }))
}

async fn kill_task(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    axum::extract::Path((client_id, task_id)): axum::extract::Path<(String, String)>,
) -> Result<Json<MessageResponse>, ApiErrorResponse> {
    let msg = ServerMsg::Kill {
        id: task_id.clone(),
    };
    state
        .send_to_client(&client_id, &msg)
        .map_err(ApiErrorResponse::not_found)?;
    info!(
        "Kill signal sent for task {} on client {}",
        task_id, client_id
    );
    Ok(Json(MessageResponse {
        message: format!("kill signal sent for task {}", task_id),
    }))
}

async fn get_task(
    _auth: AuthUser,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
) -> Result<Json<ws_shell_common::TaskResult>, ApiErrorResponse> {
    state
        .task_get(&task_id)
        .map(Json)
        .ok_or_else(|| ApiErrorResponse::not_found(format!("task {} not found or expired", task_id)))
}

// ── WebSocket: WebUI ↔ Server ────────────────────────────────────

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(mut params): Query<HashMap<String, String>>,
) -> Response {
    let token = params.remove("token").unwrap_or_default();
    ws.on_upgrade(move |socket| handle_webui_ws(socket, state, token))
}

async fn handle_webui_ws(socket: WebSocket, state: Arc<AppState>, token: String) {
    let mut authenticated = auth::verify_token(&state.jwt_secret, &token).is_some();

    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    let listener_id = uuid::Uuid::new_v4().to_string();
    state.add_webui_listener(listener_id.clone(), tx.clone());

    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sender.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    if authenticated {
        for log_msg in state.get_log_history() {
            let _ = tx.send(log_msg);
        }
    }

    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            Message::Text(text) => {
                let text: &str = &text;
                if !authenticated {
                    if let Ok(login) = serde_json::from_str::<LoginRequest>(text) {
                        if login.username == state.admin_username
                            && auth::verify_password(&state.admin_password_hash, &login.password)
                        {
                            authenticated = true;
                            let token = auth::create_token(&state.jwt_secret, &login.username);
                            let resp = LoginResponse { token };
                            let _ = tx.send(serde_json::to_string(&resp).unwrap());

                            for log_msg in state.get_log_history() {
                                let _ = tx.send(log_msg);
                            }
                            continue;
                        }
                    }
                    let _ = tx.send(
                        r#"{"error":"unauthorized","message":"invalid credentials"}"#.to_string(),
                    );
                    break;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    state.remove_webui_listener(&listener_id);
    send_task.abort();
}

// ── WebSocket: Client ↔ Server ───────────────────────────────────

async fn ws_client_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let token = params.get("token").cloned().unwrap_or_default();
    ws.on_upgrade(move |socket| handle_client_ws(socket, state, token))
}

async fn handle_client_ws(socket: WebSocket, state: Arc<AppState>, token: String) {
    // Accept both JWT tokens and raw secret for client connections
    if auth::verify_token(&state.jwt_secret, &token).is_none() && token != state.jwt_secret {
        warn!("Client connection rejected: invalid token");
        return;
    }

    let addr = "unknown".to_string();

    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_sender.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    let mut client_id: Option<String> = None;

    // Wait for registration message
    while let Some(Ok(msg)) = ws_receiver.next().await {
        if let Message::Text(text) = msg {
            let text: &str = &text;
            if let Ok(ClientMsg::Register {
                hostname, os, arch, ..
            }) = serde_json::from_str(text)
            {
                let id = uuid::Uuid::new_v4().to_string();
                info!("Client registered: {} ({})", hostname, id);
                state.add_client(id.clone(), hostname, os, arch, addr, tx.clone());
                client_id = Some(id);
                break;
            }
        }
    }

    // Main message loop
    let client_id_for_cleanup = client_id.clone();
    while let Some(Ok(msg)) = ws_receiver.next().await {
        match msg {
            Message::Text(text) => {
                let text: &str = &text;
                match serde_json::from_str::<ClientMsg>(text) {
                    Ok(client_msg) => {
                        // Enrich the JSON broadcast to WebUI with the associated client_id
                        let enriched_text = if let Some(cid) = &client_id {
                            if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(text) {
                                val["client_id"] = serde_json::Value::String(cid.clone());
                                serde_json::to_string(&val).unwrap_or_else(|_| text.to_string())
                            } else {
                                text.to_string()
                            }
                        } else {
                            text.to_string()
                        };

                        match &client_msg {
                            ClientMsg::Output { id, stream, data, .. } => {
                                info!("[{}] output: {} bytes", id, text.len());
                                let stream_str = match stream {
                                    ws_shell_common::StreamType::Stderr => "stderr",
                                    _ => "stdout",
                                };
                                state.task_append_output(id, stream_str, data);
                                state.broadcast_to_webui(&enriched_text);
                            }
                            ClientMsg::Exit { id, code } => {
                                info!("[{}] exit code: {}", id, code);
                                state.task_complete(id, *code);
                                state.broadcast_to_webui(&enriched_text);
                            }
                            ClientMsg::Error { id, msg } => {
                                warn!("[{}] error: {}", id, msg);
                                state.task_error(id, msg);
                                state.broadcast_to_webui(&enriched_text);
                            }
                            ClientMsg::Heartbeat { .. } => {
                                if let Some(cid) = &client_id {
                                    if let Some(mut client) = state.clients.get_mut(cid) {
                                        client.info.last_heartbeat =
                                            Some(chrono::Utc::now().to_rfc3339());
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    Err(e) => {
                        warn!("Invalid client message: {}", e);
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Cleanup
    if let Some(id) = client_id_for_cleanup {
        info!("Client disconnected: {}", id);
        state.remove_client(&id);
    }
    send_task.abort();
}
