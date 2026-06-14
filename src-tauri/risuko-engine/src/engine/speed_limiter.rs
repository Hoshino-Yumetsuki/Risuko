use std::sync::atomic::{AtomicU64, Ordering};

/// Token-bucket rate limiter (GCRA) for download speed control.
///
/// Lock-free and correct under contention: the entire state is a single
/// atomic `next_avail_us` (the virtual "next available time"). Each acquire
/// CAS-advances it forward by `bytes / limit` seconds; bursting is bounded
/// by clamping the start to `now - BURST_WINDOW`. Because every successful
/// acquire atomically moves `next_avail_us`, the rate is enforced exactly
/// regardless of how many concurrent callers race.
///
/// A limit of 0 means unlimited (no throttling).
pub struct SpeedLimiter {
    limit_bps: AtomicU64,
    /// Earliest virtual time (microseconds since `start`) at which the next
    /// byte can be served. Bumps forward on every successful acquire.
    next_avail_us: AtomicU64,
    start: tokio::time::Instant,
}

/// Allow up to this much burst above the steady-state rate. Matches the old
/// "2x limit" cap (2 seconds of unspent budget).
const BURST_WINDOW_US: u64 = 2_000_000;

impl SpeedLimiter {
    pub fn new(limit_bps: u64) -> Self {
        let now = tokio::time::Instant::now();
        // Pre-charge 1 second of virtual budget so the first acquire of up
        // to `limit_bps` bytes proceeds without sleeping. This matches the
        // original "tokens initialized to limit_bps" semantics.
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

    /// Try to consume `bytes` worth of tokens. Returns `Ok(())` if the
    /// virtual budget allows it now, or `Err(wait_secs)` with how long to
    /// sleep before retrying.
    fn try_consume(&self, bytes: u64, limit: u64) -> Result<(), f64> {
        // Microseconds of virtual time this request consumes. Use ceil so
        // small writes still cost at least 1us and we never under-charge.
        let cost_us = (bytes as u128 * 1_000_000).div_ceil(limit as u128) as u64;

        loop {
            let now_us = self.now_us();
            let cur = self.next_avail_us.load(Ordering::Relaxed);
            // Clamp the virtual clock so unused budget caps at BURST_WINDOW.
            let earliest = now_us.saturating_sub(BURST_WINDOW_US);
            let base = cur.max(earliest);
            let new_va = base.saturating_add(cost_us);

            // Only commit the advance when the request can be served now.
            // Committing in the wait branch double-charges: acquire would
            // sleep for `wait_secs` and then loop, recompute against the
            // already-advanced `next_avail_us`, and reserve another full
            // `cost_us` of budget on top of the bytes we already paid for.
            // For the wait branch, just report how long to sleep and let
            // the caller retry; on retry `now_us` will have advanced and a
            // single commit will succeed
            if new_va <= now_us {
                if self
                    .next_avail_us
                    .compare_exchange_weak(cur, new_va, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    return Ok(());
                }
                // Lost the race \u2014 retry.
                continue;
            }
            return Err((new_va - now_us) as f64 / 1_000_000.0);
        }
    }

    /// Acquire `bytes` worth of throughput tokens.
    /// Sleeps asynchronously if the rate limit would be exceeded.
    /// Returns immediately if limit is 0 (unlimited).
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
                // Cap sleep to 1s to stay responsive to runtime limit changes.
                Err(wait_secs) => {
                    tokio::time::sleep(std::time::Duration::from_secs_f64(wait_secs.min(1.0)))
                        .await;
                }
            }
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

    #[tokio::test]
    async fn unlimited_returns_immediately() {
        let lim = SpeedLimiter::new(0);
        // Should not block even for very large requests.
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
        // 4 MiB/s limit. First call drains the initial bucket, second call
        // must wait approximately the deficit / rate (~250ms for 1 MiB).
        let lim = SpeedLimiter::new(4 * 1024 * 1024);
        lim.acquire(4 * 1024 * 1024).await; // drain initial tokens
        let start = std::time::Instant::now();
        lim.acquire(1024 * 1024).await; // should sleep ~250ms
        let elapsed = start.elapsed();
        assert!(elapsed >= std::time::Duration::from_millis(150));
        assert!(elapsed < std::time::Duration::from_millis(800));
    }
}
