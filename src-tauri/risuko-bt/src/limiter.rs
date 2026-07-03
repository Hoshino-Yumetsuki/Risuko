//! Token-bucket rate limiter shared by every upload task in a session

use parking_lot::Mutex;
use tokio::time::Instant;

pub struct UploadLimiter {
    rate: f64,
    state: Mutex<(f64, Instant)>,
}

impl UploadLimiter {
    pub fn new(bytes_per_sec: u64) -> Self {
        let rate = bytes_per_sec.max(1) as f64;
        Self {
            rate,
            state: Mutex::new((rate, Instant::now())),
        }
    }

    pub async fn acquire(&self, n: u64) {
        let need = (n as f64).min(self.rate);
        loop {
            let wait_secs = {
                let mut st = self.state.lock();
                let now = Instant::now();
                let elapsed = now.duration_since(st.1).as_secs_f64();
                st.0 = (st.0 + elapsed * self.rate).min(self.rate);
                st.1 = now;
                if st.0 >= need {
                    st.0 -= n as f64;
                    return;
                }
                (need - st.0) / self.rate
            };
            tokio::time::sleep(std::time::Duration::from_secs_f64(wait_secs)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn acquire_paces_to_configured_rate() {
        let l = UploadLimiter::new(16 * 1024); // 16 KiB/s
        l.acquire(16 * 1024).await;
        let before = Instant::now();
        l.acquire(16 * 1024).await;
        let waited = Instant::now() - before;
        assert!(
            waited >= std::time::Duration::from_millis(900),
            "waited only {waited:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn oversized_acquire_charges_full_debt() {
        let l = UploadLimiter::new(16 * 1024); // 16 KiB/s
        l.acquire(64 * 1024).await;
        let before = Instant::now();
        l.acquire(16 * 1024).await;
        let waited = Instant::now() - before;
        assert!(
            waited >= std::time::Duration::from_millis(3900),
            "waited only {waited:?}"
        );
    }
}
