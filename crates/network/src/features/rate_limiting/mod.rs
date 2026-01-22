//! Rate limiting feature
//!
//! Prevents excessive requests to any single domain.

mod limiter;

pub use limiter::RateLimiter;
