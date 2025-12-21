//! Request rate limiting
//!
//! Prevents excessive requests to any single domain.

use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Rate limiter for HTTP requests
#[derive(Debug)]
pub struct RateLimiter {
    /// Per-domain request limits (requests per minute)
    limits: HashMap<String, u32>,
    /// Default limit for domains not explicitly configured
    default_limit: u32,
    /// Request timestamps per domain
    history: Mutex<HashMap<String, VecDeque<Instant>>>,
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
        Self {
            limits: HashMap::new(),
            default_limit: default_rpm,
            history: Mutex::new(HashMap::new()),
            window: Duration::from_secs(60),
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

        let queue = history.entry(domain_lower).or_default();

        // Remove expired entries
        while queue.front().is_some_and(|t| *t < window_start) {
            queue.pop_front();
        }

        queue.len() < limit as usize
    }

    /// Record a request to this domain
    pub fn record(&self, domain: &str) {
        let domain_lower = domain.to_lowercase();
        let mut history = self.history.lock();

        let queue = history.entry(domain_lower).or_default();
        queue.push_back(Instant::now());
    }

    /// Check and record in one operation (atomic check-then-record)
    pub fn try_acquire(&self, domain: &str) -> bool {
        let domain_lower = domain.to_lowercase();
        let limit = self.get_limit(&domain_lower);
        let mut history = self.history.lock();

        let now = Instant::now();
        let window_start = now - self.window;

        let queue = history.entry(domain_lower).or_default();

        // Remove expired entries
        while queue.front().is_some_and(|t| *t < window_start) {
            queue.pop_front();
        }

        if queue.len() < limit as usize {
            queue.push_back(now);
            true
        } else {
            false
        }
    }

    /// Get remaining requests allowed for a domain
    pub fn remaining(&self, domain: &str) -> u32 {
        let domain_lower = domain.to_lowercase();
        let limit = self.get_limit(&domain_lower);
        let mut history = self.history.lock();

        let now = Instant::now();
        let window_start = now - self.window;

        let queue = history.entry(domain_lower).or_default();

        // Remove expired entries
        while queue.front().is_some_and(|t| *t < window_start) {
            queue.pop_front();
        }

        limit.saturating_sub(queue.len() as u32)
    }

    /// Get time until next request is allowed (if rate limited)
    pub fn retry_after(&self, domain: &str) -> Option<Duration> {
        let domain_lower = domain.to_lowercase();
        let history = self.history.lock();

        if let Some(queue) = history.get(&domain_lower) {
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
}
