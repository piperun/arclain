//! Content cache feature
//!
//! Unified content cache combining cacache blob storage with SQLite index.

mod cache;
mod key_lock;
mod quota;

pub use cache::ContentCache;
pub(crate) use quota::CacheQuota;
pub use quota::{CacheCapacityRefusal, CacheLimits, CacheOwner};
