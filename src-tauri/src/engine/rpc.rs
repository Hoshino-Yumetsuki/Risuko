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

const ENGINE_VERSION: &str = "motrix-engine/0.1";

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
}

#[derive(Clone)]
struct RpcState {
    manager: Arc<TaskManager>,
    events: EventBroadcaster,
    secret: String,
    session_id: String,
}

impl RpcServer {
    pub fn new(
        host: String,
        port: u16,
        secret: String,
        manager: Arc<TaskManager>,
        events: EventBroadcaster,
    ) -> Self {
        let session_id = uuid::Uuid::new_v4()
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();

        Self {
            host,
            port,
            secret,
            session_id,
            manager,
            events,
            shutdown_tx: None,
        }
    }

    pub async fn start(&mut self) -> Result<(), String> {
        let state = RpcState {
            manager: self.manager.clone(),
            events: self.events.clone(),
            secret: self.secret.clone(),
            session_id: self.session_id.clone(),
        };

        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
            .max_age(std::time::Duration::from_secs(1728000));

        let app = Router::new()
            .route("/jsonrpc", post(handle_http_post).get(handle_http_get_or_ws))
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

        log::info!("RPC server listening on {}", addr);

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

async fn handle_http_post(
    State(state): State<RpcState>,
    body: String,
) -> Response {
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
                return json_rpc_response(rpc_error(Value::Null, INVALID_REQUEST, "Invalid Request"));
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
        Value::Object(_) => {
            match process_single_request(&state, parsed).await {
                Some(resp) => json_rpc_response(resp),
                None => (StatusCode::NO_CONTENT, "").into_response(),
            }
        }
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
    let params: HashMap<String, String> =
        url::form_urlencoded::parse(query_str.as_bytes())
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
                return json_rpc_response(rpc_error(Value::Null, INVALID_REQUEST, "Invalid Request"));
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
    let json_str = serde_json::to_string(&body).unwrap_or_default();
    match callback {
        Some(cb) if !cb.is_empty() => {
            // Sanitize callback name: allow only alphanumerics, underscore, dot
            let safe_cb: String = cb
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
                .collect();
            let jsonp = format!("{safe_cb}({json_str});");
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/javascript")],
                jsonp,
            )
                .into_response()
        }
        _ => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json-rpc")],
            json_str,
        )
            .into_response(),
    }
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

    // Auth check
    let (authed_params, auth_ok) = check_auth(&state.secret, params_vec);
    if !auth_ok {
        return Some(rpc_error(
            id.unwrap_or(Value::Null),
            1,
            "Unauthorized",
        ));
    }

    // Normalize method prefix: aria2.X → motrix.X
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
                if provided == secret {
                    params.remove(0);
                    return (params, true);
                }
            }
        }
    }
    (params, false)
}

/// Normalize `aria2.X` → `motrix.X`, pass `system.X` and `motrix.X` through
fn normalize_method(method: &str) -> String {
    if let Some(suffix) = method.strip_prefix("aria2.") {
        format!("motrix.{suffix}")
    } else {
        method.to_string()
    }
}

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
        "motrix.addUri" => {
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

            let gid = state.manager.add_http_task(uris, options).await.map_err(RpcError::from)?;
            Ok(Value::String(gid))
        }

        "motrix.addTorrent" => {
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

            let gid = state.manager.add_torrent_task(torrent_data, options).await.map_err(RpcError::from)?;
            Ok(Value::String(gid))
        }

        "motrix.addEd2k" => {
            let uri = params
                .first()
                .and_then(|v| v.as_str())
                .ok_or_else(|| RpcError::from("ed2k URI required".to_string()))?;

            let options = params
                .get(1)
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();

            let gid = state.manager.add_ed2k_task(uri, options).await.map_err(RpcError::from)?;
            Ok(Value::String(gid))
        }

        "motrix.remove" | "motrix.forceRemove" => {
            let gid = get_gid(&params)?;
            state.manager.remove(&gid).await.map_err(RpcError::from)?;
            Ok(Value::String(gid))
        }

        "motrix.pause" | "motrix.forcePause" => {
            let gid = get_gid(&params)?;
            state.manager.pause(&gid).await.map_err(RpcError::from)?;
            Ok(Value::String(gid))
        }

        "motrix.pauseAll" | "motrix.forcePauseAll" => {
            state.manager.pause_all().await;
            Ok(Value::String("OK".into()))
        }

        "motrix.unpause" => {
            let gid = get_gid(&params)?;
            state.manager.unpause(&gid).await.map_err(RpcError::from)?;
            Ok(Value::String(gid))
        }

        "motrix.unpauseAll" => {
            state.manager.unpause_all().await;
            Ok(Value::String("OK".into()))
        }

        "motrix.tellStatus" => {
            let gid = get_gid(&params)?;
            let keys = get_keys(&params, 1);
            state.manager.tell_status(&gid, &keys).await.map_err(RpcError::from)
        }

        "motrix.tellActive" => {
            let keys = get_keys(&params, 0);
            Ok(state.manager.tell_active(&keys).await)
        }

        "motrix.tellWaiting" => {
            let offset = params
                .first()
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let num = params
                .get(1)
                .and_then(|v| v.as_u64())
                .unwrap_or(1000) as usize;
            let keys = get_keys(&params, 2);
            Ok(state.manager.tell_waiting(offset, num, &keys).await)
        }

        "motrix.tellStopped" => {
            let offset = params
                .first()
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let num = params
                .get(1)
                .and_then(|v| v.as_u64())
                .unwrap_or(1000) as usize;
            let keys = get_keys(&params, 2);
            Ok(state.manager.tell_stopped(offset, num, &keys).await)
        }

        "motrix.getGlobalStat" => Ok(state.manager.get_global_stat().await),

        "motrix.changeOption" => {
            let gid = get_gid(&params)?;
            let opts = params
                .get(1)
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            state.manager.change_option(&gid, opts).await.map_err(RpcError::from)?;
            Ok(Value::String("OK".into()))
        }

        "motrix.changeGlobalOption" => {
            let opts = params
                .first()
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            state.manager.change_global_option(opts).await;
            Ok(Value::String("OK".into()))
        }

        "motrix.getOption" => {
            let gid = get_gid(&params)?;
            state.manager.get_option(&gid).await.map_err(RpcError::from)
        }

        "motrix.getGlobalOption" => Ok(state.manager.get_global_option().await),

        "motrix.changePosition" => {
            let gid = get_gid(&params)?;
            let pos = params
                .get(1)
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let how = params
                .get(2)
                .and_then(|v| v.as_str())
                .unwrap_or("POS_SET");
            state.manager.change_position(&gid, pos, how).await.map_err(RpcError::from)
        }

        "motrix.getPeers" => {
            let gid = get_gid(&params)?;
            Ok(state.manager.get_peers(&gid).await)
        }

        "motrix.getUris" => {
            let gid = get_gid(&params)?;
            state.manager.get_uris(&gid).await.map_err(RpcError::from)
        }

        "motrix.getFiles" => {
            let gid = get_gid(&params)?;
            state.manager.get_files(&gid).await.map_err(RpcError::from)
        }

        "motrix.getServers" => {
            let gid = get_gid(&params)?;
            state.manager.get_servers(&gid).await.map_err(RpcError::from)
        }

        "motrix.getSessionInfo" => {
            Ok(json!({ "sessionId": state.session_id }))
        }

        "motrix.shutdown" => {
            state.manager.save_session().await.ok();
            state.manager.shutdown().await;
            Ok(Value::String("OK".into()))
        }

        "motrix.forceShutdown" => {
            state.manager.shutdown().await;
            Ok(Value::String("OK".into()))
        }

        "motrix.saveSession" => {
            state.manager.save_session().await.map_err(RpcError::from)?;
            Ok(Value::String("OK".into()))
        }

        "motrix.purgeDownloadResult" => {
            state.manager.purge_download_result().await;
            Ok(Value::String("OK".into()))
        }

        "motrix.removeDownloadResult" => {
            let gid = get_gid(&params)?;
            state.manager.remove_download_result(&gid).await.map_err(RpcError::from)?;
            Ok(Value::String("OK".into()))
        }

        "motrix.getVersion" => Ok(json!({
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

        "system.multicall" => {
            let methods = params
                .first()
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

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

                let (clean_params, _) = check_auth(&state.secret, call_params);
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
    let motrix_methods = [
        "addUri",
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
    ];

    let mut all: Vec<Value> = Vec::new();
    for m in &motrix_methods {
        all.push(Value::String(format!("aria2.{m}")));
        all.push(Value::String(format!("motrix.{m}")));
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
        all.push(Value::String(format!("motrix.{n}")));
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
