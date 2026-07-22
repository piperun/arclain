//! Request rate limiting
//!
//! Prevents excessive requests to any single domain.

use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// The sliding window over which `requests_per_minute` is measured.
/// Named so the connection between the per-domain RPM limit and the
/// 60-second budget is explicit, rather than buried in a literal.
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct RequestHistory {
    by_scope: HashMap<String, VecDeque<Instant>>,
    last_sweep: Instant,
}

impl RequestHistory {
    fn new(now: Instant) -> Self {
        Self {
            by_scope: HashMap::new(),
            last_sweep: now,
        }
    }

    fn sweep_expired(&mut self, window_start: Instant) {
        for queue in self.by_scope.values_mut() {
            prune_expired(queue, window_start);
        }
        self.by_scope.retain(|_, queue| !queue.is_empty());
    }

    fn sweep_if_due(&mut self, now: Instant, window: Duration) {
        if now.duration_since(self.last_sweep) >= window {
            self.sweep_expired(now - window);
            self.last_sweep = now;
        }
    }
}

fn prune_expired(queue: &mut VecDeque<Instant>, window_start: Instant) {
    while queue
        .front()
        .is_some_and(|timestamp| *timestamp <= window_start)
    {
        queue.pop_front();
    }
}

/// Rate limiter for HTTP requests
#[derive(Debug)]
pub struct RateLimiter {
    /// Per-domain request limits (requests per minute)
    limits: HashMap<String, u32>,
    /// Default limit for domains not explicitly configured
    default_limit: u32,
    /// Request timestamps per domain
    history: Mutex<RequestHistory>,
    /// Window duration (default: 1 minute)
    window: Duration,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(60) // 60 requests per minute default
    }
}

impl RateLimiter {
    /// Create a new rate limiter with the given default limit
    pub fn new(default_rpm: u32) -> Self {
        let now = Instant::now();
        Self {
            limits: HashMap::new(),
            default_limit: default_rpm,
            history: Mutex::new(RequestHistory::new(now)),
            window: RATE_LIMIT_WINDOW,
        }
    }

    /// Set a custom limit for a specific domain
    pub fn set_limit(&mut self, domain: &str, requests_per_minute: u32) {
        self.limits
            .insert(domain.to_lowercase(), requests_per_minute);
    }

    /// Get the limit for a domain
    pub fn get_limit(&self, domain: &str) -> u32 {
        let domain_lower = domain.to_lowercase();
        *self
            .limits
            .get(&domain_lower)
            .unwrap_or(&self.default_limit)
    }

    /// Check if a request to this domain is allowed
    pub fn check(&self, domain: &str) -> bool {
        let domain_lower = domain.to_lowercase();
        let limit = self.get_limit(&domain_lower);
        let mut history = self.history.lock();

        let now = Instant::now();
        let window_start = now - self.window;
        history.sweep_if_due(now, self.window);
        let mut remove_scope = false;
        let request_count = history
            .by_scope
            .get_mut(&domain_lower)
            .map(|queue| {
                prune_expired(queue, window_start);
                remove_scope = queue.is_empty();
                queue.len()
            })
            .unwrap_or(0);
        if remove_scope {
            history.by_scope.remove(&domain_lower);
        }

        request_count < limit as usize
    }

    /// Record a request to this domain
    pub fn record(&self, domain: &str) {
        let domain_lower = domain.to_lowercase();
        let mut history = self.history.lock();
        let now = Instant::now();
        history.sweep_if_due(now, self.window);
        history
            .by_scope
            .entry(domain_lower)
            .or_default()
            .push_back(now);
    }

    /// Check and record in one operation (atomic check-then-record)
    pub fn try_acquire(&self, domain: &str) -> bool {
        let domain_lower = domain.to_lowercase();
        let limit = self.get_limit(&domain_lower);
        let mut history = self.history.lock();

        let now = Instant::now();
        let window_start = now - self.window;
        history.sweep_if_due(now, self.window);

        if limit == 0 {
            return false;
        }

        let queue = history.by_scope.entry(domain_lower).or_default();

        // Remove expired entries
        prune_expired(queue, window_start);

        if queue.len() < limit as usize {
            queue.push_back(now);
            true
        } else {
            false
        }
    }

    /// Atomically acquire from a caller-supplied budget for an isolated scope.
    ///
    /// Plugin callers use a `plugin_id + NUL + effective_domain` scope so one
    /// plugin cannot consume another plugin's allowance for the same domain.
    pub fn try_acquire_with_limit(&self, scope: &str, limit: u32) -> bool {
        if limit == 0 {
            return false;
        }
        let scope = scope
            .split_once('\0')
            .map(|(plugin_id, domain)| format!("{plugin_id}\0{}", domain.to_ascii_lowercase()))
            .unwrap_or_else(|| scope.to_string());
        let mut history = self.history.lock();
        let now = Instant::now();
        let window_start = now - self.window;
        history.sweep_if_due(now, self.window);
        let queue = history.by_scope.entry(scope).or_default();

        prune_expired(queue, window_start);

        if queue.len() >= limit as usize {
            return false;
        }

        queue.push_back(now);
        true
    }

    /// Get remaining requests allowed for a domain
    pub fn remaining(&self, domain: &str) -> u32 {
        let domain_lower = domain.to_lowercase();
        let limit = self.get_limit(&domain_lower);
        let mut history = self.history.lock();

        let now = Instant::now();
        let window_start = now - self.window;
        history.sweep_if_due(now, self.window);
        let mut remove_scope = false;
        let request_count = history
            .by_scope
            .get_mut(&domain_lower)
            .map(|queue| {
                prune_expired(queue, window_start);
                remove_scope = queue.is_empty();
                queue.len()
            })
            .unwrap_or(0);
        if remove_scope {
            history.by_scope.remove(&domain_lower);
        }

        limit.saturating_sub(request_count as u32)
    }

    /// Get time until next request is allowed (if rate limited)
    pub fn retry_after(&self, domain: &str) -> Option<Duration> {
        let domain_lower = domain.to_lowercase();
        let history = self.history.lock();

        if let Some(queue) = history.by_scope.get(&domain_lower) {
            if let Some(oldest) = queue.front() {
                let expires = *oldest + self.window;
                let now = Instant::now();
                if expires > now {
                    return Some(expires - now);
                }
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_limit() {
        let limiter = RateLimiter::new(5);
        assert_eq!(limiter.get_limit("example.com"), 5);
    }

    #[test]
    fn test_custom_limit() {
        let mut limiter = RateLimiter::new(60);
        limiter.set_limit("dlsite.com", 30);

        assert_eq!(limiter.get_limit("dlsite.com"), 30);
        assert_eq!(limiter.get_limit("other.com"), 60);
    }

    #[test]
    fn test_rate_limiting() {
        let limiter = RateLimiter::new(3);

        assert!(limiter.try_acquire("example.com"));
        assert!(limiter.try_acquire("example.com"));
        assert!(limiter.try_acquire("example.com"));
        assert!(!limiter.try_acquire("example.com")); // Should be denied

        // Different domain should still work
        assert!(limiter.try_acquire("other.com"));
    }

    #[test]
    fn test_remaining() {
        let limiter = RateLimiter::new(5);

        assert_eq!(limiter.remaining("example.com"), 5);
        limiter.record("example.com");
        assert_eq!(limiter.remaining("example.com"), 4);
    }

    #[test]
    fn zero_plugin_budget_does_not_allocate_history() {
        let limiter = RateLimiter::new(60);

        assert!(!limiter.try_acquire_with_limit("plugin-a\0example.com", 0));
        assert!(
            limiter.history.lock().by_scope.is_empty(),
            "zero-RPM denial left a permanent scope entry"
        );
    }

    #[test]
    fn read_only_checks_and_zero_host_budget_do_not_allocate_history() {
        let limiter = RateLimiter::new(0);

        assert!(!limiter.check("unused.example"));
        assert_eq!(limiter.remaining("unused.example"), 0);
        assert!(!limiter.try_acquire("unused.example"));
        assert!(
            limiter.history.lock().by_scope.is_empty(),
            "non-recording operations left empty history entries"
        );
    }

    #[test]
    fn plugin_budget_sweep_evicts_expired_inactive_scopes() {
        let limiter = RateLimiter::new(60);
        {
            let mut history = limiter.history.lock();
            history.by_scope.insert(
                "stale-plugin\0stale.example".to_string(),
                VecDeque::from([Instant::now() - RATE_LIMIT_WINDOW - Duration::from_secs(1)]),
            );
            history.last_sweep = Instant::now() - RATE_LIMIT_WINDOW;
        }

        assert!(limiter.try_acquire_with_limit("fresh-plugin\0fresh.example", 1));

        let history = limiter.history.lock();
        assert!(!history.by_scope.contains_key("stale-plugin\0stale.example"));
        assert_eq!(
            history.by_scope.len(),
            1,
            "expired plugin scopes were not evicted"
        );
    }
}
