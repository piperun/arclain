//! Request feature
//!
//! Provides async HTTP request functionality.

mod client;
pub mod types;

#[cfg(test)]
mod tests;

pub use client::AsyncHttpClient;
pub use types::{HttpRequest, RequestId, RequestStatus};
