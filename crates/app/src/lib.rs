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
//! [`settings`] adds the settings/secrets/vault surface: the
//! `SettingsSnapshot`/`SettingsPatch` DTOs, password-rule and
//! organization-profile summaries, and the `ArclainApp` methods declared
//! in `crate::runtime`'s own delimited "Task 10" section.
//!
//! [`plugins`] exposes renderer-neutral plugin sessions: see
//! [`plugins::PluginUiDocument`] and the facade methods
//! `ArclainApp::{plugins, set_plugin_enabled, open_plugin_session,
//! plugin_ui_document, close_plugin_session, start_plugin_action,
//! set_active_archive_session, read_plugin_image}`. It also owns the
//! domain-access surface a frontend used to reach `arclain-network`
//! directly for: [`plugins::DomainWhitelistEntryDto`] (via
//! `ArclainApp::plugin_domain_whitelist`) and the pure [`analyze_url`]
//! re-exported below.
//!
//! [`layout`] owns the application's own chrome-layout surface: the
//! arrangeable toolbar/context-menu/tools-dialog/info-panel items
//! ([`layout::UiItemDto`], via `ArclainApp::{list_ui_items,
//! save_ui_items}`) and the display options stored beside them
//! ([`layout::UiDisplayOptionsDto`], via `ArclainApp::{ui_display_options,
//! save_ui_display_options}`). Its DTOs mirror what `arclain_core`
//! re-exports from the storage layer, so a frontend draws its chrome from
//! this crate's vocabulary rather than the database's.
//!
//! [`archive::multipart`] and [`operations::merge`] between them own the
//! split-archive feature: [`archive::detect_multipart`] answers "is this
//! file part of a multi-part set?" for a frontend's drop/file-picker
//! branch, and `ArclainApp::start_merge` combines a detected set into one
//! archive as a registered operation.
//!
//! [`organization`] owns the organization feature's own surface:
//! archive-profile and organization-rule CRUD, the output formats a
//! profile may name ([`organization::archive_format_options`]), plus the
//! synchronous [`organization::OrganizePlanPreview`] an organize panel
//! recomputes as the user changes rules. The `OrganizeRequest` that
//! actually *runs* an organize stays in [`operations`], alongside every
//! other operation request -- and binds the session it previewed, so
//! what a panel applies is the plan it showed (see
//! [`operations::OrganizeRequest::archive_session_id`]).

pub mod archive;
pub mod challenge;
pub mod error;
pub mod event;
pub mod ids;
pub mod layout;
pub mod materialization;
pub mod operations;
pub mod organization;
pub mod plugins;
pub mod runtime;
pub mod settings;

pub use runtime::{AppPaths, ArclainApp, BootstrapConfig};

/// Re-exported at the crate root because it is not an application method
/// at all: [`analyze_url`] needs no `ArclainApp`, no runtime, and no I/O
/// (see its own doc comment), so requiring a caller to reach it through
/// the `plugins` module would suggest a coupling to plugin state that
/// does not exist. `arclain_app::plugins::analyze_url` still resolves too.
pub use plugins::analyze_url;

/// Re-exported so `arclain_ui` (and any other frontend) never needs
/// `arclain_signals` as a direct dependency just to hold reactive state
/// -- `arclain_signals` is a headless crate under `scripts/
/// frontend_boundary.py`'s rules, so only this facade may depend on it
/// directly. `Effect` is deliberately not re-exported: nothing outside
/// `arclain_signals` itself uses it today.
pub use arclain_signals::{Computed, Signal};

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
