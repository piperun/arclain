//! Per-plugin file logger with rate limiting and size cap.
//!
//! Plugins that misbehave (infinite loops, debug spam) get throttled
//! at the host so they can't fill the disk or drown out arclain's
//! own logs. Drop policy is silent + periodic summary written to
//! arclain.log every `SUMMARY_INTERVAL`.

use parking_lot::Mutex;
use std::time::Instant;

#[cfg(test)]
mod tests;

/// Simple token bucket. `rate_per_sec` tokens are added per second up
/// to `capacity`. `try_take` consumes one token if available.
pub(crate) struct TokenBucket {
    rate_per_sec: f64,
    capacity: f64,
    state: Mutex<BucketState>,
}

struct BucketState {
    tokens: f64,
    last_refill: Instant,
}

impl TokenBucket {
    pub(crate) fn new(rate_per_sec: f64, capacity: u32) -> Self {
        Self {
            rate_per_sec,
            capacity: capacity as f64,
            state: Mutex::new(BucketState {
                tokens: capacity as f64,
                last_refill: Instant::now(),
            }),
        }
    }

    pub(crate) fn try_take(&self) -> bool {
        let mut s = self.state.lock();
        let now = Instant::now();
        let elapsed = now.duration_since(s.last_refill).as_secs_f64();
        s.tokens = (s.tokens + elapsed * self.rate_per_sec).min(self.capacity);
        s.last_refill = now;
        if s.tokens >= 1.0 {
            s.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}
