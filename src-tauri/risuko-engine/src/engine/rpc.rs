use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequest, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use base64::Engine as _;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

use super::events::EventBroadcaster;
use super::manager::TaskManager;

const ENGINE_VERSION: &str = concat!("risuko-engine/", env!("CARGO_PKG_VERSION"));

// JSON-RPC 2.0 error codes
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

pub struct RpcServer {
    host: String,
    port: u16,
    secret: String,
    session_id: String,
    manager: Arc<TaskManager>,
    events: EventBroadcaster,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    rpc_shutdown_tx: tokio::sync::mpsc::Sender<()>,
}

#[derive(Clone)]
struct RpcState {
    manager: Arc<TaskManager>,
    events: EventBroadcaster,
    secret: String,
    session_id: String,
    rpc_shutdown_tx: tokio::sync::mpsc::Sender<()>,
}

impl RpcServer {
    pub fn new(
        host: String,
        port: u16,
        secret: String,
        manager: Arc<TaskManager>,
        events: EventBroadcaster,
        rpc_shutdown_tx: tokio::sync::mpsc::Sender<()>,
    ) -> Self {
        let session_id = uuid::Uuid::new_v4().simple().to_string();

        Self {
            host,
            port,
            secret,
            session_id,
            manager,
            events,
            shutdown_tx: None,
            rpc_shutdown_tx,
        }
    }

    pub async fn start(&mut self) -> Result<(), String> {
        let state = RpcState {
            manager: self.manager.clone(),
            events: self.events.clone(),
            secret: self.secret.clone(),
            session_id: self.session_id.clone(),
            rpc_shutdown_tx: self.rpc_shutdown_tx.clone(),
        };

        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
            .max_age(std::time::Duration::from_secs(1728000));

        let app = Router::new()
            .route(
                "/jsonrpc",
                post(handle_http_post).get(handle_http_get_or_ws),
            )
            .layer(cors)
            .with_state(state);

        let addr: SocketAddr = format!("{}:{}", self.host, self.port)
            .parse()
            .map_err(|e| format!("Invalid RPC address {}:{}: {}", self.host, self.port, e))?;
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("Failed to bind RPC port {}: {}", self.port, e))?;

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        self.shutdown_tx = Some(tx);

        tracing::info!("RPC server listening on {}", addr);

        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await
                .ok();
        });

        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

// HTTP POST handler

async fn handle_http_post(State(state): State<RpcState>, body: String) -> Response {
    let parsed = match serde_json::from_str::<Value>(&body) {
        Ok(v) => v,
        Err(_) => {
            let err = rpc_error(Value::Null, PARSE_ERROR, "Parse error");
            return json_rpc_response(err);
        }
    };

    match parsed {
        Value::Array(batch) => {
            if batch.is_empty() {
                return json_rpc_response(rpc_error(
                    Value::Null,
                    INVALID_REQUEST,
                    "Invalid Request",
                ));
            }
            let mut results = Vec::with_capacity(batch.len());
            for item in batch {
                let resp = process_single_request(&state, item).await;
                if let Some(r) = resp {
                    results.push(r);
                }
            }
            if results.is_empty() {
                // All were notifications, no response per spec
                (StatusCode::NO_CONTENT, "").into_response()
            } else {
                json_rpc_response(Value::Array(results))
            }
        }
        Value::Object(_) => match process_single_request(&state, parsed).await {
            Some(resp) => json_rpc_response(resp),
            None => (StatusCode::NO_CONTENT, "").into_response(),
        },
        _ => json_rpc_response(rpc_error(Value::Null, INVALID_REQUEST, "Invalid Request")),
    }
}

// HTTP GET handler — WebSocket upgrade or query-param RPC

async fn handle_http_get_or_ws(
    State(state): State<RpcState>,
    req: axum::extract::Request,
) -> Response {
    // Check if this is a WebSocket upgrade
    let is_upgrade = req
        .headers()
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);

    if is_upgrade {
        // Extract WebSocket upgrade from the request
        let ws = match WebSocketUpgrade::from_request(req, &state).await {
            Ok(ws) => ws,
            Err(e) => return e.into_response(),
        };
        return ws
            .on_upgrade(move |socket| handle_ws_connection(state, socket))
            .into_response();
    }

    // Parse query params from URI
    let query_str = req.uri().query().unwrap_or("");
    let params: HashMap<String, String> = url::form_urlencoded::parse(query_str.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    handle_get_query(state, params).await
}

async fn handle_get_query(state: RpcState, params: HashMap<String, String>) -> Response {
    let method = params.get("method").map(|s| s.as_str()).unwrap_or("");
    let id = params
        .get("id")
        .map(|s| Value::String(s.clone()))
        .unwrap_or(Value::Null);
    let callback = params.get("jsoncallback").cloned();

    let rpc_params = if let Some(encoded) = params.get("params") {
        match base64::engine::general_purpose::STANDARD.decode(encoded) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(s) => serde_json::from_str::<Value>(&s).unwrap_or(Value::Array(Vec::new())),
                Err(_) => Value::Array(Vec::new()),
            },
            Err(_) => Value::Array(Vec::new()),
        }
    } else {
        Value::Array(Vec::new())
    };

    // If no method specified, treat params as batch of requests
    if method.is_empty() && id == Value::Null {
        if let Value::Array(batch) = rpc_params {
            if batch.is_empty() {
                return json_rpc_response(rpc_error(
                    Value::Null,
                    INVALID_REQUEST,
                    "Invalid Request",
                ));
            }
            let mut results = Vec::with_capacity(batch.len());
            for item in batch {
                if let Some(r) = process_single_request(&state, item).await {
                    results.push(r);
                }
            }
            return maybe_jsonp(Value::Array(results), callback);
        }
    }

    // Build a JSON-RPC request from query params
    let request = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": rpc_params,
        "id": id,
    });

    let response = match process_single_request(&state, request).await {
        Some(r) => r,
        None => Value::Null,
    };

    maybe_jsonp(response, callback)
}

fn json_rpc_response(body: Value) -> Response {
    let json_str = serde_json::to_string(&body).unwrap_or_default();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json-rpc")],
        json_str,
    )
        .into_response()
}

fn maybe_jsonp(body: Value, callback: Option<String>) -> Response {
    // Sanitize callback name: allow only alphanumerics, underscore, dot
    let safe_cb: String = callback
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
        .collect();
    if safe_cb.is_empty() {
        return json_rpc_response(body);
    }
    let json_str = serde_json::to_string(&body).unwrap_or_default();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/javascript")],
        format!("{safe_cb}({json_str});"),
    )
        .into_response()
}

// WebSocket handler for batch/single/notification push

async fn handle_ws_connection(state: RpcState, mut socket: WebSocket) {
    let mut event_rx = state.events.subscribe();

    loop {
        tokio::select! {
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let parsed = match serde_json::from_str::<Value>(&text) {
                            Ok(v) => v,
                            Err(_) => {
                                let err = rpc_error(Value::Null, PARSE_ERROR, "Parse error");
                                let text = serde_json::to_string(&err).unwrap_or_default();
                                if socket.send(Message::Text(text.into())).await.is_err() {
                                    break;
                                }
                                continue;
                            }
                        };

                        match parsed {
                            Value::Array(batch) => {
                                let mut results = Vec::with_capacity(batch.len());
                                for item in batch {
                                    if let Some(r) = process_single_request(&state, item).await {
                                        results.push(r);
                                    }
                                }
                                if !results.is_empty() {
                                    let text = serde_json::to_string(&Value::Array(results)).unwrap_or_default();
                                    if socket.send(Message::Text(text.into())).await.is_err() {
                                        break;
                                    }
                                }
                            }
                            _ => {
                                if let Some(resp) = process_single_request(&state, parsed).await {
                                    let text = serde_json::to_string(&resp).unwrap_or_default();
                                    if socket.send(Message::Text(text.into())).await.is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
            event = event_rx.recv() => {
                match event {
                    Ok(event) => {
                        let notification = event.to_notification();
                        let text = serde_json::to_string(&notification).unwrap_or_default();
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => break,
                }
            }
        }
    }
}

/// Process a single JSON-RPC request object
/// Returns `None` for notifications (requests without `id`)
async fn process_single_request(state: &RpcState, request: Value) -> Option<Value> {
    let id = request.get("id").cloned();

    let method = match request.get("method").and_then(|v| v.as_str()) {
        Some(m) if !m.is_empty() => m,
        _ => {
            return Some(rpc_error(
                id.unwrap_or(Value::Null),
                INVALID_REQUEST,
                "Invalid Request",
            ));
        }
    };

    let params = request
        .get("params")
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));

    let params_vec = match params {
        Value::Array(v) => v,
        Value::Object(_) => {
            return Some(rpc_error(
                id.unwrap_or(Value::Null),
                INVALID_PARAMS,
                "Named params not supported",
            ));
        }
        _ => vec![params],
    };

    // aria2 special-case: system.multicall does not require outer auth
    // Each nested call is authenticated independently
    let (authed_params, auth_ok) = if method == "system.multicall" {
        (params_vec, true)
    } else {
        check_auth(&state.secret, params_vec)
    };
    if !auth_ok {
        return Some(rpc_error(id.unwrap_or(Value::Null), 1, "Unauthorized"));
    }

    // Normalize method prefix: aria2.X → risuko.X
    let normalized = normalize_method(method);

    let result = dispatch_method(state, &normalized, authed_params).await;

    // If no id, this is a notification — don't send response
    let id = match id {
        Some(v) => v,
        None => return None,
    };

    Some(match result {
        Ok(value) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": value,
        }),
        Err(RpcError { code, message }) => rpc_error(id, code, &message),
    })
}

fn check_auth(secret: &str, mut params: Vec<Value>) -> (Vec<Value>, bool) {
    if secret.is_empty() {
        // Still strip token param if provided, for compatibility
        if let Some(first) = params.first() {
            if let Some(s) = first.as_str() {
                if s.starts_with("token:") {
                    params.remove(0);
                }
            }
        }
        return (params, true);
    }

    if let Some(first) = params.first() {
        if let Some(token_str) = first.as_str() {
            if let Some(provided) = token_str.strip_prefix("token:") {
                if secret_eq(provided, secret) {
                    params.remove(0);
                    return (params, true);
                }
            }
        }
    }
    (params, false)
}

/// Constant-time comparison for RPC tokens
///
/// Hashes both sides to fixed-size digests, then fold-XORs to avoid early exit
fn secret_eq(provided: &str, secret: &str) -> bool {
    use sha2::{Digest, Sha256};
    let a = Sha256::digest(provided.as_bytes());
    let b = Sha256::digest(secret.as_bytes());
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Normalize `aria2.X` → `risuko.X`, pass `system.X` and `risuko.X` through
fn normalize_method(method: &str) -> String {
    if let Some(suffix) = method.strip_prefix("aria2.") {
        format!("risuko.{suffix}")
    } else {
        method.to_string()
    }
}

/// Extract the nested multicall list from either the standard `[[calls]]`
/// shape or the legacy `["token:...", [calls]]` shape
///
/// This helper only returns the inner call array. It does not validate or use
/// the outer `"token:..."` value for authentication; outer auth is
/// intentionally ignored for `system.multicall`, and each nested call is
/// authenticated independently. Malformed shapes return an empty `Vec`, so the
/// `starts_with("token:")` check here is shape detection, not auth logic
fn extract_multicall_methods(params: &[Value]) -> Vec<Value> {
    // Accept both formats:
    // 1) standard aria2: [ [ { methodName, params } ] ]
    // 2) backward-compatible legacy: [ "token:...", [ { methodName, params } ] ]
    params
        .first()
        .and_then(|v| v.as_array())
        .or_else(|| {
            params.get(1).and_then(|v| v.as_array()).filter(|_| {
                params
                    .first()
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| s.starts_with("token:"))
            })
        })
        .cloned()
        .unwrap_or_default()
}

#[derive(Debug)]
struct RpcError {
    code: i64,
    message: String,
}

impl From<String> for RpcError {
    fn from(message: String) -> Self {
        Self { code: 1, message }
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        }
    })
}

// Dispatches a normalized method name to the corresponding manager function
fn dispatch_method<'a>(
    state: &'a RpcState,
    method: &'a str,
    params: Vec<Value>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, RpcError>> + Send + 'a>> {
    Box::pin(async move {
        match method {
            "risuko.addUri" => {
                let uris = params
                    .first()
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();

                if uris.is_empty() {
                    return Err("URI required".to_string().into());
                }

                let options = params
                    .get(1)
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default();

                let gid = state
                    .manager
                    .add_http_task(uris, options)
                    .await
                    .map_err(RpcError::from)?;
                Ok(Value::String(gid))
            }

            "risuko.addMedia" => {
                let uri = params
                    .first()
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| RpcError::from("Media URL required".to_string()))?
                    .trim();

                if uri.is_empty() {
                    return Err(RpcError::from("Media URL required".to_string()));
                }

                let options = params
                    .get(1)
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default();

                if !super::media::is_media_uri(uri) && !super::media::is_force_ytdlp(&options) {
                    return Err(RpcError::from("Not a supported media URL".to_string()));
                }

                let gid = state
                    .manager
                    .add_media_task(uri, options)
                    .await
                    .map_err(RpcError::from)?;
                Ok(Value::String(gid))
            }

            "risuko.addTorrent" => {
                let torrent_b64 = params
                    .first()
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| RpcError::from("Torrent data required".to_string()))?;

                let torrent_data = base64::engine::general_purpose::STANDARD
                    .decode(torrent_b64)
                    .map_err(|e| RpcError::from(format!("Invalid base64: {e}")))?;

                // aria2 convention: params[1] = uris (unused by us), params[2] = options
                let options = params
                    .get(2)
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default();

                let gid = state
                    .manager
                    .add_torrent_task(torrent_data, options)
                    .await
                    .map_err(RpcError::from)?;
                Ok(Value::String(gid))
            }

            "risuko.addMetalink" => {
                let metalink_b64 = params
                    .first()
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| RpcError::from("Metalink data required".to_string()))?;

                let metalink_data = base64::engine::general_purpose::STANDARD
                    .decode(metalink_b64)
                    .map_err(|e| RpcError::from(format!("Invalid base64: {e}")))?;

                let options = params
                    .get(1)
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default();

                let gid = state
                    .manager
                    .add_metalink_task(metalink_data, options)
                    .await
                    .map_err(RpcError::from)?;
                Ok(Value::String(gid))
            }

            "risuko.addNzb" => {
                let nzb_b64 = params
                    .first()
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| RpcError::from("NZB data required".to_string()))?;
                let nzb_data = base64::engine::general_purpose::STANDARD
                    .decode(nzb_b64)
                    .map_err(|e| RpcError::from(format!("Invalid base64: {e}")))?;
                let options = params
                    .get(1)
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default();
                let gid = state
                    .manager
                    .add_nzb_task(nzb_data, options)
                    .await
                    .map_err(RpcError::from)?;
                Ok(Value::String(gid))
            }

            "risuko.addEd2k" => {
                let uri = params
                    .first()
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| RpcError::from("ed2k URI required".to_string()))?;

                let options = params
                    .get(1)
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default();

                let gid = state
                    .manager
                    .add_ed2k_task(uri, options)
                    .await
                    .map_err(RpcError::from)?;
                Ok(Value::String(gid))
            }

            "risuko.remove" | "risuko.forceRemove" => {
                let gid = resolve_gid(&params, &state.manager).await?;
                state.manager.remove(&gid).await.map_err(RpcError::from)?;
                Ok(Value::String(gid))
            }

            "risuko.pause" | "risuko.forcePause" => {
                let gid = resolve_gid(&params, &state.manager).await?;
                state.manager.pause(&gid).await.map_err(RpcError::from)?;
                Ok(Value::String(gid))
            }

            "risuko.pauseAll" | "risuko.forcePauseAll" => {
                state.manager.pause_all().await;
                Ok(Value::String("OK".into()))
            }

            "risuko.unpause" => {
                let gid = resolve_gid(&params, &state.manager).await?;
                state.manager.unpause(&gid).await.map_err(RpcError::from)?;
                Ok(Value::String(gid))
            }

            "risuko.unpauseAll" => {
                state.manager.unpause_all().await;
                Ok(Value::String("OK".into()))
            }

            "risuko.tellStatus" => {
                let gid = resolve_gid(&params, &state.manager).await?;
                let keys = get_keys(&params, 1);
                state
                    .manager
                    .tell_status(&gid, &keys)
                    .await
                    .map_err(RpcError::from)
            }

            "risuko.tellActive" => {
                let keys = get_keys(&params, 0);
                Ok(state.manager.tell_active(&keys).await)
            }

            "risuko.tellWaiting" => {
                let offset = params.first().and_then(|v| v.as_i64()).unwrap_or(0);
                let num = params.get(1).and_then(|v| v.as_u64()).unwrap_or(5000) as usize;
                let keys = get_keys(&params, 2);
                Ok(state.manager.tell_waiting(offset, num, &keys).await)
            }

            "risuko.tellStopped" => {
                let offset = params.first().and_then(|v| v.as_i64()).unwrap_or(0);
                let num = params.get(1).and_then(|v| v.as_u64()).unwrap_or(5000) as usize;
                let keys = get_keys(&params, 2);
                Ok(state.manager.tell_stopped(offset, num, &keys).await)
            }

            "risuko.getGlobalStat" => Ok(state.manager.get_global_stat().await),

            "risuko.changeOption" => {
                let gid = resolve_gid(&params, &state.manager).await?;
                let opts = params
                    .get(1)
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default();
                state
                    .manager
                    .change_option(&gid, opts)
                    .await
                    .map_err(RpcError::from)?;
                Ok(Value::String("OK".into()))
            }

            "risuko.changeGlobalOption" => {
                let opts = params
                    .first()
                    .and_then(|v| v.as_object())
                    .cloned()
                    .unwrap_or_default();
                state.manager.change_global_option(opts).await;
                Ok(Value::String("OK".into()))
            }

            "risuko.getOption" => {
                let gid = resolve_gid(&params, &state.manager).await?;
                state.manager.get_option(&gid).await.map_err(RpcError::from)
            }

            "risuko.getGlobalOption" => Ok(state.manager.get_global_option().await),

            "risuko.changePosition" => {
                let gid = resolve_gid(&params, &state.manager).await?;
                let pos = params.get(1).and_then(|v| v.as_i64()).unwrap_or(0);
                let how = params.get(2).and_then(|v| v.as_str()).unwrap_or("POS_SET");
                state
                    .manager
                    .change_position(&gid, pos, how)
                    .await
                    .map_err(RpcError::from)
            }

            "risuko.getPeers" => {
                let gid = resolve_gid(&params, &state.manager).await?;
                Ok(state.manager.get_peers(&gid).await)
            }

            "risuko.getUris" => {
                let gid = resolve_gid(&params, &state.manager).await?;
                state.manager.get_uris(&gid).await.map_err(RpcError::from)
            }

            "risuko.getFiles" => {
                let gid = resolve_gid(&params, &state.manager).await?;
                state.manager.get_files(&gid).await.map_err(RpcError::from)
            }

            "risuko.getServers" => {
                let gid = resolve_gid(&params, &state.manager).await?;
                state
                    .manager
                    .get_servers(&gid)
                    .await
                    .map_err(RpcError::from)
            }

            "risuko.getSessionInfo" => Ok(json!({ "sessionId": state.session_id })),

            "risuko.shutdown" => {
                state.manager.save_session().await.ok();
                let _ = state.rpc_shutdown_tx.send(()).await;
                Ok(Value::String("OK".into()))
            }

            "risuko.forceShutdown" => {
                let _ = state.rpc_shutdown_tx.send(()).await;
                Ok(Value::String("OK".into()))
            }

            "risuko.saveSession" => {
                state.manager.save_session().await.map_err(RpcError::from)?;
                Ok(Value::String("OK".into()))
            }

            "risuko.purgeDownloadResult" => {
                state.manager.purge_download_result().await;
                Ok(Value::String("OK".into()))
            }

            "risuko.removeDownloadResult" => {
                let gid = resolve_gid(&params, &state.manager).await?;
                state
                    .manager
                    .remove_download_result(&gid)
                    .await
                    .map_err(RpcError::from)?;
                Ok(Value::String("OK".into()))
            }

            "risuko.getVersion" => Ok(json!({
                "version": ENGINE_VERSION,
                "enabledFeatures": [
                    "HTTP",
                    "HTTPS",
                    "FTP",
                    "FTPS",
                    "SFTP",
                    "BitTorrent",
                    "JSON-RPC",
                ]
            })),

            "risuko.listRoutingRules" => {
                let rules = state.manager.list_routing_rules().await;
                Ok(serde_json::to_value(rules).unwrap_or_default())
            }

            "risuko.addRoutingRule" => {
                let rule_value = params
                    .into_iter()
                    .next()
                    .ok_or_else(|| RpcError::from("Invalid routing rule".to_string()))?;
                let rule = serde_json::from_value::<super::routing::TaskRoutingRule>(rule_value)
                    .map_err(|e| RpcError::from(format!("Invalid routing rule: {e}")))?;
                let added = state
                    .manager
                    .add_routing_rule(rule)
                    .await
                    .map_err(RpcError::from)?;
                Ok(serde_json::to_value(added).unwrap_or_default())
            }

            "risuko.updateRoutingRule" => {
                let rule_value = params
                    .into_iter()
                    .next()
                    .ok_or_else(|| RpcError::from("Invalid routing rule".to_string()))?;
                let rule = serde_json::from_value::<super::routing::TaskRoutingRule>(rule_value)
                    .map_err(|e| RpcError::from(format!("Invalid routing rule: {e}")))?;
                state
                    .manager
                    .update_routing_rule(rule)
                    .await
                    .map_err(RpcError::from)?;
                Ok(Value::String("OK".into()))
            }

            "risuko.removeRoutingRule" => {
                let id = params
                    .first()
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| RpcError::from("Rule ID required".to_string()))?;
                state
                    .manager
                    .remove_routing_rule(id)
                    .await
                    .map_err(RpcError::from)?;
                Ok(Value::String("OK".into()))
            }

            "risuko.resolveRouting" => {
                let filename = params
                    .first()
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| RpcError::from("Filename required".to_string()))?;
                let decision = state.manager.preview_routing(filename).await;
                Ok(serde_json::to_value(decision).unwrap_or_default())
            }

            "system.multicall" => {
                let methods = extract_multicall_methods(&params);

                let mut results = Vec::with_capacity(methods.len());
                for call in methods {
                    let method_name = call
                        .get("methodName")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let call_params = call
                        .get("params")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();

                    let (clean_params, auth_ok) = check_auth(&state.secret, call_params);
                    if !auth_ok {
                        results.push(json!({
                            "code": 1,
                            "message": "Unauthorized",
                        }));
                        continue;
                    }

                    let normalized = normalize_method(method_name);

                    match dispatch_method(state, &normalized, clean_params).await {
                        Ok(value) => results.push(Value::Array(vec![value])),
                        Err(e) => results.push(json!({
                            "code": e.code,
                            "message": e.message,
                        })),
                    }
                }
                Ok(Value::Array(results))
            }

            "system.listMethods" => Ok(list_methods()),

            "system.listNotifications" => Ok(list_notifications()),

            _ => Err(RpcError {
                code: METHOD_NOT_FOUND,
                message: format!("Method not found: {method}"),
            }),
        }
    })
}

// Method & notification listings
fn list_methods() -> Value {
    let risuko_methods = [
        "addUri",
        "addMedia",
        "addTorrent",
        "addEd2k",
        "remove",
        "forceRemove",
        "pause",
        "forcePause",
        "pauseAll",
        "forcePauseAll",
        "unpause",
        "unpauseAll",
        "tellStatus",
        "tellActive",
        "tellWaiting",
        "tellStopped",
        "getGlobalStat",
        "changeOption",
        "changeGlobalOption",
        "getOption",
        "getGlobalOption",
        "changePosition",
        "getPeers",
        "getUris",
        "getFiles",
        "getServers",
        "getSessionInfo",
        "shutdown",
        "forceShutdown",
        "saveSession",
        "purgeDownloadResult",
        "removeDownloadResult",
        "getVersion",
        "listRoutingRules",
        "addRoutingRule",
        "updateRoutingRule",
        "removeRoutingRule",
        "resolveRouting",
    ];

    let mut all: Vec<Value> = Vec::new();
    for m in &risuko_methods {
        all.push(Value::String(format!("aria2.{m}")));
        all.push(Value::String(format!("risuko.{m}")));
    }
    all.push(Value::String("system.multicall".into()));
    all.push(Value::String("system.listMethods".into()));
    all.push(Value::String("system.listNotifications".into()));
    Value::Array(all)
}

fn list_notifications() -> Value {
    let names = [
        "onDownloadStart",
        "onDownloadPause",
        "onDownloadStop",
        "onDownloadComplete",
        "onDownloadError",
        "onBtDownloadComplete",
    ];

    let mut all: Vec<Value> = Vec::new();
    for n in &names {
        all.push(Value::String(format!("aria2.{n}")));
        all.push(Value::String(format!("risuko.{n}")));
    }
    Value::Array(all)
}

// Helpers

fn get_gid(params: &[Value]) -> Result<String, RpcError> {
    params
        .first()
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| RpcError::from("GID required".to_string()))
}

/// Parse a GID from params and resolve prefix to full GID via the manager
async fn resolve_gid(params: &[Value], manager: &TaskManager) -> Result<String, RpcError> {
    let prefix = get_gid(params)?;
    manager.resolve_gid(&prefix).await.map_err(RpcError::from)
}

fn get_keys(params: &[Value], index: usize) -> Vec<String> {
    params
        .get(index)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    async fn make_rpc_state(secret: &str) -> (RpcState, TempDir) {
        let events = EventBroadcaster::default();
        let config_dir = TempDir::new().unwrap();
        let options = super::super::options::EngineOptions::from_config(
            &serde_json::Map::new(),
            &serde_json::Map::new(),
        );
        let manager = Arc::new(
            TaskManager::new(config_dir.path(), options, events.clone())
                .await
                .unwrap(),
        );
        let (rpc_shutdown_tx, _rpc_shutdown_rx) = tokio::sync::mpsc::channel(1);

        (
            RpcState {
                manager,
                events,
                secret: secret.to_string(),
                session_id: "test-session".to_string(),
                rpc_shutdown_tx,
            },
            config_dir,
        )
    }

    // -- check_auth --

    #[test]
    fn auth_empty_secret_always_ok() {
        let params = vec![json!("hello")];
        let (out, ok) = check_auth("", params);
        assert!(ok);
        assert_eq!(out, vec![json!("hello")]);
    }

    #[test]
    fn auth_empty_secret_strips_token() {
        let params = vec![json!("token:abc"), json!("arg1")];
        let (out, ok) = check_auth("", params);
        assert!(ok);
        assert_eq!(out, vec![json!("arg1")]);
    }

    #[test]
    fn auth_valid_token() {
        let params = vec![json!("token:mysecret"), json!("gid1")];
        let (out, ok) = check_auth("mysecret", params);
        assert!(ok);
        assert_eq!(out, vec![json!("gid1")]);
    }

    #[test]
    fn auth_wrong_token() {
        let params = vec![json!("token:wrong"), json!("gid1")];
        let (out, ok) = check_auth("mysecret", params);
        assert!(!ok);
        assert_eq!(out.len(), 2); // params untouched
    }

    #[test]
    fn auth_missing_token_param() {
        let params = vec![json!("gid1")];
        let (out, ok) = check_auth("mysecret", params);
        assert!(!ok);
        assert_eq!(out, vec![json!("gid1")]);
    }

    #[test]
    fn auth_non_string_first_param() {
        let params = vec![json!(42), json!("gid1")];
        let (_, ok) = check_auth("mysecret", params);
        assert!(!ok);
    }

    #[test]
    fn auth_empty_params_with_secret() {
        let params = vec![];
        let (out, ok) = check_auth("mysecret", params);
        assert!(!ok);
        assert!(out.is_empty());
    }

    // -- normalize_method --

    #[test]
    fn normalize_aria2_to_risuko() {
        assert_eq!(normalize_method("aria2.addUri"), "risuko.addUri");
        assert_eq!(normalize_method("aria2.tellStatus"), "risuko.tellStatus");
    }

    #[test]
    fn normalize_passthrough() {
        assert_eq!(normalize_method("risuko.addUri"), "risuko.addUri");
        assert_eq!(normalize_method("system.listMethods"), "system.listMethods");
    }

    // -- multicall helpers --

    #[test]
    fn extract_multicall_methods_standard_shape() {
        let call = json!({
            "methodName": "aria2.tellActive",
            "params": ["token:secret", ["gid"]]
        });
        let params = vec![json!([call.clone()])];
        let methods = extract_multicall_methods(&params);
        assert_eq!(methods, vec![call]);
    }

    #[test]
    fn extract_multicall_methods_legacy_shape() {
        let call = json!({
            "methodName": "aria2.tellActive",
            "params": ["token:secret", ["gid"]]
        });
        let params = vec![json!("token:secret"), json!([call.clone()])];
        let methods = extract_multicall_methods(&params);
        assert_eq!(methods, vec![call]);
    }

    #[test]
    fn extract_multicall_methods_rejects_second_arg_without_token_prefix() {
        let call = json!({
            "methodName": "aria2.tellActive",
            "params": ["token:secret", ["gid"]]
        });
        let params = vec![json!("secret"), json!([call])];
        let methods = extract_multicall_methods(&params);
        assert!(methods.is_empty());
    }

    #[tokio::test]
    async fn multicall_mixed_nested_auth_standard_shape_keeps_processing() {
        let (state, _config_dir) = make_rpc_state("secret").await;
        let request = json!({
            "jsonrpc": "2.0",
            "id": "1",
            "method": "system.multicall",
            "params": [[
                {
                    "methodName": "aria2.getVersion",
                    "params": []
                },
                {
                    "methodName": "aria2.getVersion",
                    "params": ["token:secret"]
                }
            ]]
        });

        let response = process_single_request(&state, request).await.unwrap();
        let results = response.get("result").and_then(|v| v.as_array()).unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].get("code").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(
            results[0].get("message").and_then(|v| v.as_str()),
            Some("Unauthorized")
        );

        let success = results[1].as_array().unwrap();
        let version_obj = success.first().and_then(|v| v.as_object()).unwrap();
        assert_eq!(
            version_obj.get("version").and_then(|v| v.as_str()),
            Some(ENGINE_VERSION)
        );
    }

    #[tokio::test]
    async fn multicall_mixed_nested_auth_legacy_shape_keeps_processing() {
        let (state, _config_dir) = make_rpc_state("secret").await;
        let request = json!({
            "jsonrpc": "2.0",
            "id": "1",
            "method": "system.multicall",
            "params": [
                "token:secret",
                [
                    {
                        "methodName": "aria2.getVersion",
                        "params": ["token:wrong"]
                    },
                    {
                        "methodName": "aria2.getVersion",
                        "params": ["token:secret"]
                    }
                ]
            ]
        });

        let response = process_single_request(&state, request).await.unwrap();
        let results = response.get("result").and_then(|v| v.as_array()).unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].get("code").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(
            results[0].get("message").and_then(|v| v.as_str()),
            Some("Unauthorized")
        );

        let success = results[1].as_array().unwrap();
        let version_obj = success.first().and_then(|v| v.as_object()).unwrap();
        assert_eq!(
            version_obj.get("version").and_then(|v| v.as_str()),
            Some(ENGINE_VERSION)
        );
    }

    // -- get_gid --

    #[test]
    fn get_gid_ok() {
        let params = vec![json!("abc123")];
        assert_eq!(get_gid(&params).unwrap(), "abc123");
    }

    #[test]
    fn get_gid_empty_params() {
        let params: Vec<Value> = vec![];
        assert!(get_gid(&params).is_err());
    }

    #[test]
    fn get_gid_non_string() {
        let params = vec![json!(42)];
        assert!(get_gid(&params).is_err());
    }

    // -- get_keys --

    #[test]
    fn get_keys_extracts_strings() {
        let params = vec![json!("gid"), json!(["status", "totalLength"])];
        let keys = get_keys(&params, 1);
        assert_eq!(keys, vec!["status", "totalLength"]);
    }

    #[test]
    fn get_keys_missing_index_returns_empty() {
        let params = vec![json!("gid")];
        let keys = get_keys(&params, 1);
        assert!(keys.is_empty());
    }

    #[test]
    fn get_keys_non_array_returns_empty() {
        let params = vec![json!("gid"), json!("not_array")];
        let keys = get_keys(&params, 1);
        assert!(keys.is_empty());
    }
}
