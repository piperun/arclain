//! Request feature
//!
//! Provides async HTTP request functionality.

mod client;
pub mod types;

pub use client::AsyncHttpClient;
pub use types::{HttpRequest, RequestId, RequestStatus};
