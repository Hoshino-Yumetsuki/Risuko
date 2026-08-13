use std::sync::atomic::{AtomicU64, Ordering};

/// Lock-free token-bucket (GCRA) download rate limiter: state is one atomic `next_avail_us` (virtual next-available time) CAS-advanced by `bytes / limit` seconds per acquire, bursting bounded by clamping start to `now - BURST_WINDOW`; limit 0 means unlimited
pub struct SpeedLimiter {
    limit_bps: AtomicU64,
    /// Earliest virtual time (microseconds since `start`) the next byte can be served; bumps forward on every successful acquire
    next_avail_us: AtomicU64,
    start: tokio::time::Instant,
}

/// Max burst above steady-state rate, matching the old "2x limit" cap (2 seconds of unspent budget)
const BURST_WINDOW_US: u64 = 2_000_000;

impl SpeedLimiter {
    pub fn new(limit_bps: u64) -> Self {
        let now = tokio::time::Instant::now();
        // Pre-charge 1 second of virtual budget so the first acquire of up to `limit_bps` bytes proceeds without sleeping, matching the original "tokens initialized to limit_bps" semantics
        let start = now
            .checked_sub(std::time::Duration::from_secs(1))
            .unwrap_or(now);
        Self {
            limit_bps: AtomicU64::new(limit_bps),
            next_avail_us: AtomicU64::new(0),
            start,
        }
    }

    /// Update the speed limit at runtime (bytes per second, 0 = unlimited)
    pub fn set_limit(&self, bps: u64) {
        self.limit_bps.store(bps, Ordering::Relaxed);
    }

    pub fn limit_bps(&self) -> u64 {
        self.limit_bps.load(Ordering::Relaxed)
    }

    fn now_us(&self) -> u64 {
        tokio::time::Instant::now()
            .saturating_duration_since(self.start)
            .as_micros() as u64
    }

    /// Try to consume `bytes` of tokens: `Ok(())` if the virtual budget allows now, else `Err(wait_secs)` telling how long to sleep before retrying
    fn try_consume(&self, bytes: u64, limit: u64) -> Result<(), f64> {
        // Microseconds of virtual time this request consumes; ceil so small writes cost at least 1us and never under-charge
        let cost_us = (bytes as u128 * 1_000_000).div_ceil(limit as u128) as u64;

        loop {
            let now_us = self.now_us();
            let cur = self.next_avail_us.load(Ordering::Relaxed);
            // Clamp the virtual clock so unused budget caps at BURST_WINDOW
            let earliest = now_us.saturating_sub(BURST_WINDOW_US);
            let base = cur.max(earliest);
            let new_va = base.saturating_add(cost_us);

            // Commit the advance only when servable now: committing in the wait branch double-charges (sleep, loop, recompute against the already-advanced `next_avail_us`, reserve another `cost_us`), so wait just reports the sleep and lets the caller retry once `now_us` advances
            if new_va <= now_us {
                if self
                    .next_avail_us
                    .compare_exchange_weak(cur, new_va, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    return Ok(());
                }
                // Lost the race \u2014 retry
                continue;
            }
            return Err((new_va - now_us) as f64 / 1_000_000.0);
        }
    }

    /// Acquire `bytes` of throughput tokens, sleeping if the rate limit would be exceeded; returns immediately when limit is 0 (unlimited)
    pub async fn acquire(&self, bytes: usize) {
        if bytes == 0 {
            return;
        }
        let mut remaining = bytes as u64;
        while remaining > 0 {
            let limit = self.limit_bps.load(Ordering::Relaxed);
            if limit == 0 {
                return;
            }
            let take = remaining.min(limit);
            match self.try_consume(take, limit) {
                Ok(()) => remaining -= take,
                // Cap sleep to 1s to stay responsive to runtime limit changes
                Err(wait_secs) => {
                    tokio::time::sleep(std::time::Duration::from_secs_f64(wait_secs.min(1.0)))
                        .await;
                }
            }
        }
    }
}

/// Parse a speed limit ("5M", "10K", "128k", "0", or numeric) into bytes/sec, where 0 means unlimited and K/M are 1024/1048576
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

#[derive(Default)]
pub struct SpeedEma {
    ema: f64,
    initialized: bool,
}

impl SpeedEma {
    /// Smoothing factor: higher reacts faster to bursts, lower is smoother
    const ALPHA: f64 = 0.3;

    pub fn new() -> Self {
        Self::default()
    }

    /// Feed `bytes` transferred over `elapsed_secs` (must be > 0) and return the smoothed speed in bytes/sec (truncated)
    pub fn update(&mut self, bytes: u64, elapsed_secs: f64) -> u64 {
        let instant = bytes as f64 / elapsed_secs;
        self.ema = if self.initialized {
            Self::ALPHA * instant + (1.0 - Self::ALPHA) * self.ema
        } else {
            self.initialized = true;
            instant
        };
        self.ema as u64
    }

    /// Current smoothed speed in bytes/sec (truncated, same as [`Self::update`])
    pub fn get(&self) -> u64 {
        self.ema as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn speed_ema_seeds_on_first_sample() {
        let mut ema = SpeedEma::new();
        assert_eq!(ema.update(1000, 1.0), 1000);
        let next = ema.update(2000, 1.0);
        assert!((1000..2000).contains(&next), "got {next}");
    }

    #[test]
    fn speed_ema_does_not_reseed_after_decay_below_one() {
        let mut ema = SpeedEma::new();
        assert_eq!(ema.update(1, 10.0), 0); // 0.1 B/s
        assert_eq!(ema.get(), 0);
        let next = ema.update(1000, 1.0);
        assert!(next > 0 && next < 1000, "expected EMA blend, got {next}");
    }

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

    #[tokio::test]
    async fn unlimited_returns_immediately() {
        let lim = SpeedLimiter::new(0);
        let start = std::time::Instant::now();
        lim.acquire(10 * 1024 * 1024).await;
        assert!(start.elapsed() < std::time::Duration::from_millis(100));
    }

    #[tokio::test(start_paused = true)]
    async fn oversized_acquire_eventually_completes() {
        let lim = SpeedLimiter::new(1024);
        tokio::time::timeout(std::time::Duration::from_secs(60), lim.acquire(8 * 1024))
            .await
            .expect("oversized acquire must not hang");
    }

    #[tokio::test]
    async fn limited_throttles_then_refills() {
        // 4 MiB/s limit: first call drains the initial bucket, second call must wait ~ deficit / rate (~250ms for 1 MiB)
        let lim = SpeedLimiter::new(4 * 1024 * 1024);
        lim.acquire(4 * 1024 * 1024).await; // drain initial tokens
        let start = std::time::Instant::now();
        lim.acquire(1024 * 1024).await; // should sleep ~250ms
        let elapsed = start.elapsed();
        assert!(elapsed >= std::time::Duration::from_millis(150));
        assert!(elapsed < std::time::Duration::from_millis(800));
    }
}
