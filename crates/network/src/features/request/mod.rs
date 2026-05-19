//! Request feature
//!
//! Provides async HTTP request functionality.

mod client;
pub mod types;

#[cfg(test)]
mod tests;

pub use client::{AsyncHttpClient, StreamingDownload};
pub use types::{HttpRequest, RequestId, RequestStatus};
