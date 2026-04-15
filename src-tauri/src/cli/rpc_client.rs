use serde_json::{json, Value};

pub struct RpcClient {
    url: String,
    secret: Option<String>,
    client: reqwest::Client,
    id_counter: std::sync::atomic::AtomicU64,
}

impl RpcClient {
    pub fn new(port: u16, secret: Option<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap_or_else(|e| {
                log::warn!("Failed to build HTTP client with custom config: {e}, using defaults");
                reqwest::Client::default()
            });
        Self {
            url: format!("http://127.0.0.1:{}/jsonrpc", port),
            secret,
            client,
            id_counter: std::sync::atomic::AtomicU64::new(1),
        }
    }

    pub async fn call(&self, method: &str, params: Vec<Value>) -> Result<Value, RpcError> {
        let id = self
            .id_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let params = if let Some(ref secret) = self.secret {
            let mut with_token = vec![json!(format!("token:{}", secret))];
            with_token.extend(params);
            with_token
        } else {
            params
        };

        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let resp = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .map_err(|e| RpcError::Connection(e.to_string()))?;

        let result: Value = resp
            .json()
            .await
            .map_err(|e| RpcError::Parse(e.to_string()))?;

        if let Some(error) = result.get("error") {
            let message = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown RPC error");
            return Err(RpcError::Rpc(message.to_string()));
        }

        result
            .get("result")
            .cloned()
            .ok_or_else(|| RpcError::Parse("Missing 'result' in RPC response".to_string()))
    }

    pub async fn is_engine_running(&self) -> bool {
        self.call("risuko.getVersion", vec![]).await.is_ok()
    }
}

#[derive(Debug)]
pub enum RpcError {
    Connection(String),
    Parse(String),
    Rpc(String),
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcError::Connection(e) => write!(f, "Connection error: {}", e),
            RpcError::Parse(e) => write!(f, "Parse error: {}", e),
            RpcError::Rpc(e) => write!(f, "RPC error: {}", e),
        }
    }
}

impl std::error::Error for RpcError {}
