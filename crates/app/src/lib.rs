//! Stable, frontend-neutral application facade types for Arclain.
//!
//! `arclain_app` is the single crate both the egui frontend and a future
//! Flutter frontend depend on instead of reaching into headless crate
//! internals directly. It must stay usable without any GUI toolkit --
//! nothing here may depend on egui, eframe, or a Flutter/Dart bridge.
//!
//! This task introduces only the identifier ([`ids`]), error-envelope
//! ([`error`]), and archive read-model ([`archive`]) types. The remaining
//! modules from the full facade contract (`challenge`, `event`,
//! `materialization`, `operations`, `plugins`, `runtime`, `settings`) are
//! added incrementally by later tasks; this crate declares only the
//! modules it actually implements so far.

pub mod archive;
pub mod error;
pub mod ids;

/// The application facade's own compatibility version, independent of the
/// crate's Cargo package version. Frontends (egui today, Flutter later)
/// check this to detect a facade upgrade that changes behavior they need
/// to account for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApplicationApiVersion {
    pub major: u16,
    pub minor: u16,
}

pub const APPLICATION_API_VERSION: ApplicationApiVersion =
    ApplicationApiVersion { major: 1, minor: 0 };
