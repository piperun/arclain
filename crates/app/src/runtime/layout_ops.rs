//! `AppRuntime`-touching execution logic for the chrome-layout facade
//! surface: reading and writing the arrangeable items of a region, and
//! the display options stored beside them.
//!
//! `crate::layout` holds the DTOs and the pure validation/conversion this
//! module calls into; `crate::runtime`'s own `impl ArclainApp` exposes the
//! thin dispatch wrappers -- the same layering
//! `crate::organization`/`runtime::organization_ops` uses.
//!
//! ## Where the rows live
//!
//! `ui_items` and `ui_display_options` are two tables in the same config
//! database every setting, organization rule and archive profile already
//! lives in, reached through the one pooled handle
//! `ConfigDbs::config_pool` -- here specifically via
//! `arclain_core::services::UiService`, which wraps that same pool.
//! Nothing here opens its own connection, so a write and the read that
//! follows it cannot contend across two independent connections to one
//! file.
//!
//! ## Serializing mutations
//!
//! Because those rows share a store with settings, the mutating functions
//! here take `AppRuntime::settings_write_lock` for their whole duration,
//! exactly as `runtime::settings_ops` and `runtime::organization_ops` do.
//! A layout save is a *batch* of upserts and a display-options save is
//! six independent key writes, so without the lock a second concurrent
//! save could interleave its statements with the first and leave the
//! stored layout a blend of two different arrangements -- SQLite
//! serializes individual statements, not sequences of them. Read-only
//! functions never take it.
//!
//! ## Last write wins, deliberately
//!
//! Neither save carries a revision or a compare-and-swap, because the
//! storage underneath never had one: `UiService::upsert_items` is an
//! unconditional `INSERT .. ON CONFLICT DO UPDATE` per item and
//! `set_display_option` is the same for one key. Two frontends editing
//! one region concurrently therefore end with whichever saved last, which
//! is what the pre-facade layout editor already did. This is *not*
//! `update_settings`'s optimistic-revision contract, and it is not
//! silently weaker than the storage it wraps -- see
//! `ArclainApp::save_ui_items`'s own doc comment, which states it for
//! callers.
//!
//! ## A save never deletes
//!
//! `run_save_ui_items` upserts the batch it is given and leaves every
//! other row in the region alone. That is load-bearing rather than
//! incidental: a layout editor filters host-managed items out of the list
//! it offers the user (and therefore out of the list it saves), so a save
//! that treated its list as the region's complete contents would delete
//! those rows the first time the user pressed Save. Hiding an item is
//! expressed as `visible: false`, never as absence.

use std::sync::Arc;

use arclain_core::services::UiService;

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};
use crate::layout::{self, UiDisplayOptionsDto, UiItemDto, UiRegionDto, UiViewModeDto};

use super::AppRuntime;

// ============================================================================
// Shared resolution helpers.
// ============================================================================

fn ui_service(inner: &Arc<AppRuntime>) -> Option<Arc<UiService>> {
    inner.core_services().ui_service.clone()
}

fn require_ui_service(inner: &Arc<AppRuntime>) -> Result<Arc<UiService>, ApplicationError> {
    ui_service(inner).ok_or_else(layout_unavailable_error)
}

/// This application's own runtime handle -- never the caller's ambient
/// one, per the crate's runtime rules.
fn handle_for(inner: &Arc<AppRuntime>) -> Result<tokio::runtime::Handle, ApplicationError> {
    inner.tokio_handle().ok_or_else(shutdown_mid_request_error)
}

// ============================================================================
// Items.
// ============================================================================

/// Every stored item of `region`, in `sort_order`.
///
/// Empty (not an error) when no configuration database is open, matching
/// `run_organization_rules`'s identical treatment of the same situation:
/// "this region has no items configured" is a truthful answer, and the
/// pre-facade startup path -- which skipped the load entirely when there
/// was no service -- left the same empty layout behind.
pub(super) async fn run_list_ui_items(
    inner: &Arc<AppRuntime>,
    region: UiRegionDto,
) -> Result<Vec<UiItemDto>, ApplicationError> {
    let Some(service) = ui_service(inner) else {
        return Ok(Vec::new());
    };
    let handle = handle_for(inner)?;
    let items = handle
        .spawn_blocking(move || service.list_items(region.into()))
        .await
        .map_err(internal_join_error)?
        .map_err(|error| backend_error("listing layout items", error))?;
    Ok(items.into_iter().map(UiItemDto::from).collect())
}

pub(super) async fn run_save_ui_items(
    inner: &Arc<AppRuntime>,
    region: UiRegionDto,
    items: Vec<UiItemDto>,
) -> Result<(), ApplicationError> {
    // Structural validation first: a malformed batch never reaches the
    // write lock, let alone the database.
    let items = layout::items_to_core(region, items)?;
    if items.is_empty() {
        // Nothing to write, and nothing a concurrent writer could be
        // confused by -- so this does not take the write lock either.
        return Ok(());
    }

    let _write_guard = inner.settings_write_lock.lock().await;
    let service = require_ui_service(inner)?;
    handle_for(inner)?
        .spawn_blocking(move || service.upsert_items(&items))
        .await
        .map_err(internal_join_error)?
        .map_err(|error| backend_error("saving layout items", error))
}

// ============================================================================
// Display options.
// ============================================================================

/// The stored display options, with any option that has never been set
/// reading as its default (see [`UiDisplayOptionsDto`]).
///
/// `Unsupported` -- not defaults -- when no configuration database is
/// open, which is where this deliberately differs from
/// [`run_list_ui_items`]. An empty item list is a truthful answer to
/// "what is in this region"; a defaults struct is *not* a truthful answer
/// to "what are this user's display options", and a caller that stored it
/// as if it were would then act on preferences the user never chose.
pub(super) async fn run_ui_display_options(
    inner: &Arc<AppRuntime>,
) -> Result<UiDisplayOptionsDto, ApplicationError> {
    let service = require_ui_service(inner)?;
    let handle = handle_for(inner)?;
    handle
        .spawn_blocking(move || read_display_options(&service))
        .await
        .map_err(internal_join_error)?
}

/// Six pooled single-key reads. Deliberately left as six rather than
/// reaching past `UiService` for one multi-key query: this runs when a
/// settings page opens, the pool hands out an already-open connection,
/// and one access path to these rows is worth more than six microseconds.
fn read_display_options(service: &UiService) -> Result<UiDisplayOptionsDto, ApplicationError> {
    let defaults = UiDisplayOptionsDto::default();
    let read = |key: &'static str| -> Result<Option<String>, ApplicationError> {
        service
            .get_display_option(key)
            .map_err(|error| backend_error("reading a layout display option", error))
    };

    Ok(UiDisplayOptionsDto {
        default_view_mode: read(layout::DEFAULT_VIEW_MODE_KEY)?
            .map(|value| UiViewModeDto::from_stored(&value))
            .unwrap_or(defaults.default_view_mode),
        tree_panel_visible: layout::stored_bool(
            read(layout::TREE_PANEL_VISIBLE_KEY)?,
            defaults.tree_panel_visible,
        ),
        tree_panel_width: layout::stored_width(
            read(layout::TREE_PANEL_WIDTH_KEY)?,
            defaults.tree_panel_width,
        ),
        properties_panel_visible: layout::stored_bool(
            read(layout::PROPERTIES_PANEL_VISIBLE_KEY)?,
            defaults.properties_panel_visible,
        ),
        properties_panel_width: layout::stored_width(
            read(layout::PROPERTIES_PANEL_WIDTH_KEY)?,
            defaults.properties_panel_width,
        ),
        show_button_labels: layout::stored_bool(
            read(layout::SHOW_BUTTON_LABELS_KEY)?,
            defaults.show_button_labels,
        ),
    })
}

pub(super) async fn run_save_ui_display_options(
    inner: &Arc<AppRuntime>,
    options: UiDisplayOptionsDto,
) -> Result<(), ApplicationError> {
    layout::check_display_options(&options)?;

    let _write_guard = inner.settings_write_lock.lock().await;
    let service = require_ui_service(inner)?;
    handle_for(inner)?
        .spawn_blocking(move || write_display_options(&service, options))
        .await
        .map_err(internal_join_error)?
}

fn write_display_options(
    service: &UiService,
    options: UiDisplayOptionsDto,
) -> Result<(), ApplicationError> {
    let writes = [
        (
            layout::DEFAULT_VIEW_MODE_KEY,
            options.default_view_mode.as_stored().to_string(),
        ),
        (
            layout::TREE_PANEL_VISIBLE_KEY,
            options.tree_panel_visible.to_string(),
        ),
        (
            layout::TREE_PANEL_WIDTH_KEY,
            options.tree_panel_width.to_string(),
        ),
        (
            layout::PROPERTIES_PANEL_VISIBLE_KEY,
            options.properties_panel_visible.to_string(),
        ),
        (
            layout::PROPERTIES_PANEL_WIDTH_KEY,
            options.properties_panel_width.to_string(),
        ),
        (
            layout::SHOW_BUTTON_LABELS_KEY,
            options.show_button_labels.to_string(),
        ),
    ];
    for (key, value) in writes {
        service
            .set_display_option(key, &value)
            .map_err(|error| backend_error("saving a layout display option", error))?;
    }
    Ok(())
}

// ============================================================================
// Error helpers.
// ============================================================================

fn layout_unavailable_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Unsupported,
        "layout configuration is unavailable: no configuration database is open",
    )
    .with_recoverability(Recoverability::Fatal)
}

fn backend_error(context: &'static str, error: anyhow::Error) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Backend, "layout storage failed")
        .with_diagnostic(format!("{context}: {error:#}"))
        .with_recoverability(Recoverability::Retry)
        .with_retryable(true)
}

fn shutdown_mid_request_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Internal,
        "application is shutting down",
    )
    .with_recoverability(Recoverability::Fatal)
}

fn internal_join_error(join_error: tokio::task::JoinError) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Internal, "internal task failed")
        .with_diagnostic(join_error.to_string())
}
