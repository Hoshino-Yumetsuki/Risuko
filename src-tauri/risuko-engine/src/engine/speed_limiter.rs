use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Token-bucket rate limiter for download speed control
///
/// Shared across tasks for global limiting, or per-task for individual limiting.
/// A limit of 0 means unlimited (no throttling)
pub struct SpeedLimiter {
    limit_bps: Arc<AtomicU64>,
    state: Mutex<TokenState>,
}

struct TokenState {
    tokens: f64,
    last_refill: tokio::time::Instant,
}

impl SpeedLimiter {
    pub fn new(limit_bps: u64) -> Self {
        Self {
            limit_bps: Arc::new(AtomicU64::new(limit_bps)),
            state: Mutex::new(TokenState {
                tokens: limit_bps as f64,
                last_refill: tokio::time::Instant::now(),
            }),
        }
    }

    /// Update the speed limit at runtime (bytes per second, 0 = unlimited)
    pub fn set_limit(&self, bps: u64) {
        self.limit_bps.store(bps, Ordering::Relaxed);
    }

    /// Acquire `bytes` worth of throughput tokens
    /// Blocks asynchronously if the rate limit would be exceeded
    /// Returns immediately if limit is 0 (unlimited)
    pub async fn acquire(&self, bytes: usize) {
        if bytes == 0 {
            return;
        }
        let limit = self.limit_bps.load(Ordering::Relaxed);
        if limit == 0 {
            return;
        }

        loop {
            // Reload limit each iteration so runtime changes via set_limit take effect
            let limit = self.limit_bps.load(Ordering::Relaxed);
            if limit == 0 {
                return;
            }

            let wait_secs = {
                let mut state = self.state.lock().await;
                let now = tokio::time::Instant::now();
                let elapsed = now.duration_since(state.last_refill).as_secs_f64();

                // Refill tokens based on elapsed time. Cap burst to 2x the limit
                state.tokens = (state.tokens + elapsed * limit as f64).min(limit as f64 * 2.0);
                state.last_refill = now;

                if state.tokens >= bytes as f64 {
                    state.tokens -= bytes as f64;
                    return;
                }

                // Calculate sleep time for the deficit
                let deficit = bytes as f64 - state.tokens;
                deficit / limit as f64
            };

            // Sleep outside the lock. Cap at 1 second to stay responsive to limit changes
            tokio::time::sleep(std::time::Duration::from_secs_f64(wait_secs.min(1.0))).await;
        }
    }
}

/// Parse a speed limit string like "5M", "10K", "128k", "0", or numeric value
/// Returns bytes per second. 0 means unlimited
///
/// E.g.
/// - `"0"` or `0` → 0 (unlimited)
/// - `"1024"` → 1024 bytes/sec
/// - `"10K"` or `"10k"` → 10 * 1024 = 10240 bytes/sec
/// - `"5M"` or `"5m"` → 5 * 1048576 = 5242880 bytes/sec
pub fn parse_speed_limit(value: &serde_json::Value) -> u64 {
    match value {
        serde_json::Value::Number(n) => n.as_u64().unwrap_or(0),
        serde_json::Value::String(s) => parse_speed_limit_str(s),
        _ => 0,
    }
}

fn parse_speed_limit_str(s: &str) -> u64 {
    let s = s.trim();
    if s.is_empty() || s == "0" {
        return 0;
    }

    let (num_part, multiplier) = if s.ends_with('M') || s.ends_with('m') {
        (&s[..s.len() - 1], 1024 * 1024)
    } else if s.ends_with('K') || s.ends_with('k') {
        (&s[..s.len() - 1], 1024)
    } else {
        (s, 1u64)
    };

    num_part
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
        .saturating_mul(multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_limits() {
        assert_eq!(parse_speed_limit(&json!(0)), 0);
        assert_eq!(parse_speed_limit(&json!("0")), 0);
        assert_eq!(parse_speed_limit(&json!("10K")), 10 * 1024);
        assert_eq!(parse_speed_limit(&json!("10k")), 10 * 1024);
        assert_eq!(parse_speed_limit(&json!("5M")), 5 * 1024 * 1024);
        assert_eq!(parse_speed_limit(&json!("5m")), 5 * 1024 * 1024);
        assert_eq!(parse_speed_limit(&json!("1024")), 1024);
        assert_eq!(parse_speed_limit(&json!("")), 0);
        assert_eq!(parse_speed_limit(&json!(null)), 0);
    }
}
