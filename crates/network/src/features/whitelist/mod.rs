//! Domain whitelist feature
//!
//! Manages which domains plugins are allowed to access.

pub mod types;
mod whitelist;

pub use types::{AccessCheck, WhitelistEntry};
pub use whitelist::DomainWhitelist;
