//! Request feature
//!
//! Provides async HTTP request functionality.

mod client;
mod plugin_policy;
pub mod types;

#[cfg(test)]
mod tests;

pub use client::{
    AsyncHttpClient, PreparedPluginNetworkRouting, StreamingDownload, StreamingResponseMetadata,
};
pub use plugin_policy::PluginNetworkPolicy;
pub use types::{HttpRequest, RequestId, RequestStatus};
