//! Stable, frontend-neutral application facade types for Arclain.
//!
//! `arclain_app` is the single crate both the egui frontend and a future
//! Flutter frontend depend on instead of reaching into headless crate
//! internals directly. It must stay usable without any GUI toolkit --
//! nothing here may depend on egui, eframe, or a Flutter/Dart bridge.
//!
//! This task adds the identifier ([`ids`]), error-envelope ([`error`]),
//! archive read-model ([`archive`]), secret/challenge ([`challenge`]), and
//! operation-event ([`event`]) types, plus the [`operations`] registry
//! those events are broadcast through. `operations` is declared `pub` so
//! the module path is stable per the facade contract, but everything
//! inside it is `pub(crate)`: the registry is an implementation detail
//! behind the application facade a later task adds, never a type a
//! frontend names directly. The remaining modules from the full facade
//! contract (`materialization`, `plugins`, `runtime`, `settings`) are
//! added incrementally by later tasks; this crate declares only the
//! modules it actually implements so far.

pub mod archive;
pub mod challenge;
pub mod error;
pub mod event;
pub mod ids;
pub mod operations;

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
