//! arclain-network: Secure async network client
//!
//! Provides network functionality with:
//! - Domain security analysis (homograph detection, suspicious patterns)
//! - Plugin domain whitelisting
//! - Request rate limiting
//! - Fully async, non-blocking requests
//!
//! # Example
//!
//! ```ignore
//! use arclain_network::{AsyncHttpClient, DomainWhitelist, HttpRequest};
//! use tokio::runtime::Handle;
//!
//! let whitelist = DomainWhitelist::new();
//! let client = AsyncHttpClient::new(whitelist, Handle::current());
//!
//! // For plugins (with security checks)
//! let id = client.request_for_plugin("my-plugin", HttpRequest::get("https://dlsite.com"))?;
//!
//! // Poll for result
//! loop {
//!     if let Some(status) = client.status(&id) {
//!         if status.is_complete() {
//!             break;
//!         }
//!     }
//! }
//! ```

pub mod features;
pub mod shared;

// Re-export main types at crate root
pub use features::rate_limiting::RateLimiter;
pub use features::request::{AsyncHttpClient, HttpRequest, RequestId, RequestStatus};
pub use features::security::{analyze_url, DomainInfo, DomainWarning};
pub use features::whitelist::{AccessCheck, DomainWhitelist, WhitelistEntry};
pub use shared::{HttpError, HttpMethod, HttpResponse};
