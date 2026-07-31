//! How this feature reaches the application facade.
//!
//! Every organization surface — the rules page, the profiles page, the
//! organize panel — is a render function that emits an intent plus a
//! dispatcher that runs it. The dispatchers all need the same two
//! things (the facade handle and a runtime to await it on) and all
//! report failures the same way (a string in the page's own error
//! banner), so those two steps live here instead of in three copies.
//!
//! Calls block briefly on the shared runtime rather than spawning:
//! every one of them is a small config-database read or write on an
//! already-running runtime, and a page that emitted a load intent has
//! nothing to render until the answer arrives — the same trade-off
//! `core::state::password_ops` makes for the settings page's own rule
//! saves.

use crate::shared::SharedState;
use arclain_app::error::ApplicationError;
use arclain_app::ArclainApp;
use tokio::runtime::Handle;

/// The facade and the runtime to await it on, or `None` for a fixture
/// that was built without a facade (see `SharedState::facade`).
pub fn handles(shared: &SharedState) -> Option<(&ArclainApp, &Handle)> {
    shared
        .facade
        .as_ref()
        .map(|app| (app, &*shared.services.tokio_runtime))
}

/// What a page shows when there is no facade at all. A real application
/// always has one; this is the message a test fixture or a failed
/// bootstrap surfaces instead of rendering an empty page that looks
/// like "no rules configured".
pub fn unavailable() -> String {
    "The application backend is unavailable.".to_string()
}

/// One facade error as page text. `summary` is short and already
/// safe to display; `diagnostic` is bounded and path-redacted at
/// construction, and carries the specific reason (which field, which
/// value) that `summary` alone often leaves out.
pub fn describe(context: &str, error: &ApplicationError) -> String {
    match &error.diagnostic {
        Some(diagnostic) => format!("{context}: {} ({diagnostic})", error.summary),
        None => format!("{context}: {}", error.summary),
    }
}
