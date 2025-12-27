//! Arclain UI Library
//!
//! Exposes the core, features, shared, and platform modules for integration testing.

pub mod core;
pub mod features;
pub mod platform;
pub mod shared;

// Re-export ArclainApp for convenience
pub use core::arclain_app::ArclainApp;
