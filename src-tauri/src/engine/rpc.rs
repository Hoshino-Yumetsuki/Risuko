use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Json;
use axum::Router;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

use super::events::EventBroadcaster;
use super::manager::TaskManager;

const ENGINE_VERSION: &str = "motrix-engine/0.1";

pub struct RpcServer {
    host: String,
    port: u16,
    secret: String,
    manager: Arc<TaskManager>,
    events: EventBroadcaster,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

#[derive(Clone)]
struct RpcState {
    manager: Arc<TaskManager>,
    events: EventBroadcaster,
    secret: String,
}

impl RpcServer {
    pub fn new(
        host: String,
        port: u16,
        secret: String,
        manager: Arc<TaskManager>,
        events: EventBroadcaster,
    ) -> Self {
        Self {
            host,
            port,
            secret,
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
        };

        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);

        let app = Router::new()
            .route("/jsonrpc", post(handle_http_rpc))
            .route("/jsonrpc", axum::routing::get(handle_ws_upgrade))
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

async fn handle_http_rpc(
    State(state): State<RpcState>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let response = process_rpc_request(&state, body).await;
    Json(response)
}

async fn handle_ws_upgrade(
    State(state): State<RpcState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_connection(state, socket))
}

async fn handle_ws_connection(state: RpcState, mut socket: WebSocket) {
    let mut event_rx = state.events.subscribe();

    loop {
        tokio::select! {
            // Handle incoming messages from client
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(request) = serde_json::from_str::<Value>(&text) {
                            let response = process_rpc_request(&state, request).await;
                            let response_text = serde_json::to_string(&response).unwrap_or_default();
                            if socket.send(Message::Text(response_text.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
            // Push event notifications
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

async fn process_rpc_request(state: &RpcState, request: Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let params = request
        .get("params")
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));

    let params_vec = match params {
        Value::Array(v) => v,
        _ => vec![params],
    };

    // Check auth
    let (authed_params, auth_ok) = check_auth(&state.secret, params_vec);
    if !auth_ok {
        return rpc_error(id, 1, "Unauthorized");
    }

    let result = dispatch_method(state, method, authed_params).await;

    match result {
        Ok(value) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": value,
        }),
        Err(msg) => rpc_error(id, 1, &msg),
    }
}

fn check_auth(secret: &str, mut params: Vec<Value>) -> (Vec<Value>, bool) {
    if secret.is_empty() {
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

fn dispatch_method<'a>(
    state: &'a RpcState,
    method: &'a str,
    params: Vec<Value>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, String>> + Send + 'a>> {
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
                return Err("URI required".to_string());
            }

            let options = params
                .get(1)
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();

            let gid = state.manager.add_http_task(uris, options).await?;
            Ok(Value::String(gid))
        }

        "motrix.addTorrent" => {
            let torrent_b64 = params
                .first()
                .and_then(|v| v.as_str())
                .ok_or("Torrent data required")?;

            let torrent_data = base64_decode(torrent_b64)?;

            let options = params
                .get(2)
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();

            let gid = state.manager.add_torrent_task(torrent_data, options).await?;
            Ok(Value::String(gid))
        }

        "motrix.addEd2k" => {
            let uri = params
                .first()
                .and_then(|v| v.as_str())
                .ok_or("ed2k URI required")?;

            let options = params
                .get(1)
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();

            let gid = state.manager.add_ed2k_task(uri, options).await?;
            Ok(Value::String(gid))
        }

        "motrix.remove" | "motrix.forceRemove" => {
            let gid = get_gid(&params)?;
            state.manager.remove(&gid).await?;
            Ok(Value::String(gid))
        }

        "motrix.pause" | "motrix.forcePause" => {
            let gid = get_gid(&params)?;
            state.manager.pause(&gid).await?;
            Ok(Value::String(gid))
        }

        "motrix.pauseAll" | "motrix.forcePauseAll" => {
            state.manager.pause_all().await;
            Ok(Value::String("OK".into()))
        }

        "motrix.unpause" => {
            let gid = get_gid(&params)?;
            state.manager.unpause(&gid).await?;
            Ok(Value::String(gid))
        }

        "motrix.unpauseAll" => {
            state.manager.unpause_all().await;
            Ok(Value::String("OK".into()))
        }

        "motrix.tellStatus" => {
            let gid = get_gid(&params)?;
            let keys = get_keys(&params, 1);
            state.manager.tell_status(&gid, &keys).await
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
            state.manager.change_option(&gid, opts).await?;
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
            state.manager.get_option(&gid).await
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
            state.manager.change_position(&gid, pos, how).await
        }

        "motrix.getPeers" => {
            let gid = get_gid(&params)?;
            Ok(state.manager.get_peers(&gid).await)
        }

        "motrix.saveSession" => {
            state.manager.save_session().await?;
            Ok(Value::String("OK".into()))
        }

        "motrix.purgeDownloadResult" => {
            state.manager.purge_download_result().await;
            Ok(Value::String("OK".into()))
        }

        "motrix.removeDownloadResult" => {
            let gid = get_gid(&params)?;
            state.manager.remove_download_result(&gid).await?;
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

                // Strip auth token from sub-call params (already authenticated)
                let (clean_params, _) = check_auth(&state.secret, call_params);

                match dispatch_method(state, method_name, clean_params).await {
                    Ok(value) => results.push(Value::Array(vec![value])),
                    Err(msg) => results.push(json!({
                        "code": 1,
                        "message": msg,
                    })),
                }
            }
            Ok(Value::Array(results))
        }

        "system.listMethods" => Ok(json!([
            "motrix.addUri",
            "motrix.addTorrent",
            "motrix.addEd2k",
            "motrix.remove",
            "motrix.forceRemove",
            "motrix.pause",
            "motrix.forcePause",
            "motrix.pauseAll",
            "motrix.forcePauseAll",
            "motrix.unpause",
            "motrix.unpauseAll",
            "motrix.tellStatus",
            "motrix.tellActive",
            "motrix.tellWaiting",
            "motrix.tellStopped",
            "motrix.getGlobalStat",
            "motrix.changeOption",
            "motrix.changeGlobalOption",
            "motrix.getOption",
            "motrix.getGlobalOption",
            "motrix.changePosition",
            "motrix.getPeers",
            "motrix.saveSession",
            "motrix.purgeDownloadResult",
            "motrix.removeDownloadResult",
            "motrix.getVersion",
            "system.multicall",
            "system.listMethods",
            "system.listNotifications",
        ])),

        "system.listNotifications" => Ok(json!([
            "motrix.onDownloadStart",
            "motrix.onDownloadPause",
            "motrix.onDownloadStop",
            "motrix.onDownloadComplete",
            "motrix.onDownloadError",
            "motrix.onBtDownloadComplete",
        ])),

        _ => Err(format!("Method not found: {}", method)),
    }
    })
}

fn get_gid(params: &[Value]) -> Result<String, String> {
    params
        .first()
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "GID required".to_string())
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

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    const TABLE: [u8; 128] = {
        let mut t = [255u8; 128];
        let chars = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut i = 0;
        while i < 64 {
            t[chars[i] as usize] = i as u8;
            i += 1;
        }
        t
    };

    let input = input.trim();
    if input.is_empty() {
        return Ok(Vec::new());
    }

    let bytes: Vec<u8> = input.bytes().filter(|&b| b != b'\n' && b != b'\r' && b != b' ').collect();
    let mut output = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut i = 0;

    while i + 3 < bytes.len() {
        let b0 = TABLE.get(bytes[i] as usize).copied().unwrap_or(255);
        let b1 = TABLE.get(bytes[i + 1] as usize).copied().unwrap_or(255);
        let b2 = if bytes[i + 2] == b'=' { 0 } else { TABLE.get(bytes[i + 2] as usize).copied().unwrap_or(255) };
        let b3 = if bytes[i + 3] == b'=' { 0 } else { TABLE.get(bytes[i + 3] as usize).copied().unwrap_or(255) };

        if b0 == 255 || b1 == 255 || (bytes[i + 2] != b'=' && b2 == 255) || (bytes[i + 3] != b'=' && b3 == 255) {
            return Err("Invalid base64".to_string());
        }

        output.push((b0 << 2) | (b1 >> 4));
        if bytes[i + 2] != b'=' {
            output.push(((b1 & 0x0f) << 4) | (b2 >> 2));
        }
        if bytes[i + 3] != b'=' {
            output.push(((b2 & 0x03) << 6) | b3);
        }

        i += 4;
    }

    Ok(output)
}
