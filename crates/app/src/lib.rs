//! Stable, frontend-neutral application facade types for Arclain.
//!
//! `arclain_app` is the single crate both the egui frontend and a future
//! Flutter frontend depend on instead of reaching into headless crate
//! internals directly. It must stay usable without any GUI toolkit --
//! nothing here may depend on egui, eframe, or a Flutter/Dart bridge.
//!
//! Earlier tasks added the identifier ([`ids`]), error-envelope
//! ([`error`]), archive read-model ([`archive`]), secret/challenge
//! ([`challenge`]), and operation-event ([`event`]) types, plus the
//! [`operations`] registry those events are broadcast through, and
//! [`runtime`]: the [`runtime::ArclainApp`] facade itself, which owns the
//! Tokio runtime and composes every headless service `crates/ui`'s
//! initialization used to build directly. [`materialization`] adds
//! leased, application-owned materialization of an archive entry onto a
//! real local disk path.
//!
//! `operations`'s own registry/challenge-waiter internals stay
//! `pub(crate)` -- implementation details behind the application facade,
//! never named by a frontend directly -- but it also hosts one
//! submodule per operation *kind* (starting with `operations::extract`),
//! each contributing its own genuinely public request type
//! (`operations::ExtractRequest`, and so on as later tasks add
//! `start_convert`/`start_organize`/etc.).
//!
//! The remaining modules from the full facade contract (`plugins`,
//! `settings`) are added incrementally by later tasks; this crate declares
//! only the modules it actually implements so far.

pub mod archive;
pub mod challenge;
pub mod error;
pub mod event;
pub mod ids;
pub mod materialization;
pub mod operations;
pub mod runtime;

pub use runtime::{AppPaths, ArclainApp, BootstrapConfig};

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
