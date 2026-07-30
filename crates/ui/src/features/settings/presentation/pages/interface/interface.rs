//! Interface settings page.
//!
//! Architecture: `render_interface_settings` returns
//! `Option<InterfaceSettingsAction>` describing intent — either a
//! display-options load, a display-options save, a per-item
//! visibility toggle, or a navigation request. The sibling
//! `handle_interface_settings_action` function owns every application
//! call and signal mutation so the render path itself stays a
//! pure intent-emitter.
//!
//! **Item state lives in signals, not on the page.** Toolbar /
//! info-panel / context-menu items are read from
//! `shared.signals().toolbar_items`, `info_panel_items`, and
//! `context_menu_items` respectively. Each toggle emits a
//! `ToggleItemVisibility` action; the dispatcher saves the item
//! through the application and updates the signal. The page itself no
//! longer holds an `items: Vec<UiItemDto>` cache — that used to mirror
//! the same data the LayoutEditor's state already owned, and the two
//! could drift after a save in one without the other knowing.

use crate::shared::components::Form;
use crate::shared::theme::AppTheme;
use crate::shared::SharedState;
use arclain_app::layout::{UiDisplayOptionsDto, UiRegionDto};
use arclain_theme::spacing;
use arclain_widgets::{ButtonSize, TextButton};
use eframe::egui;

use super::sections;
use crate::features::settings::application::facade;

/// State for interface settings as the application reports them.
///
/// Holds *display-option* state only. Item state (toolbar buttons,
/// info-panel sections, context-menu entries) lives on the canonical
/// `AppSignals` and is consumed via `shared.signals()`.
#[derive(Default)]
pub struct InterfaceSettingsState {
    /// Every display option in one value, which is also the shape the
    /// application reads and writes — so nothing here re-parses stored
    /// text or decides what an unset option means.
    ///
    /// Starts at the application's own fresh-profile answer, so an
    /// un-loaded page holds what a first-run load would return rather
    /// than an arbitrary placeholder.
    pub display_options: UiDisplayOptionsDto,
    /// Display-options changes since last save. Item toggles persist
    /// immediately and do not affect this flag.
    pub dirty: bool,
    /// `false` until the first LoadDisplayOptions dispatch completes.
    pub loaded: bool,
    /// The last load or save failure, shown on the page.
    ///
    /// Also a latch: the page auto-fires its load and save intents every
    /// frame, so a failure that left `loaded` false or `dirty` true would
    /// otherwise retry sixty times a second and report itself each time.
    /// While this is set, neither intent fires again; the next edit
    /// clears it and the retry resumes.
    pub error: Option<String>,
    /// Show the layout type selection dialog
    pub layout_dialog_open: bool,
}

impl InterfaceSettingsState {
    /// Read the display options through the application. Items are
    /// loaded elsewhere (state/init.rs at startup, state/config_ops.rs
    /// on reload) and live in signals — this method only touches the
    /// page-local display-option fields.
    ///
    /// `loaded` flips only on success, so a failed read leaves the page
    /// on its "Loading…" state rather than presenting placeholder values
    /// as if the user had chosen them.
    pub fn load(&mut self, shared: &SharedState) -> Result<(), String> {
        if self.loaded {
            return Ok(());
        }
        let (app, runtime) = facade::handles(shared).ok_or_else(facade::unavailable)?;
        self.display_options = runtime
            .block_on(app.ui_display_options())
            .map_err(|error| facade::describe("Failed to load interface settings", &error))?;
        self.loaded = true;
        self.dirty = false;
        Ok(())
    }

    /// Persist the display options through the application. Item toggles
    /// are not handled here — they persist per-toggle via the
    /// `ToggleItemVisibility` action.
    ///
    /// `dirty` is cleared only on success, so a failed save leaves the
    /// page pending rather than dropping the user's edit.
    pub fn save(&mut self, shared: &SharedState) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        let (app, runtime) = facade::handles(shared).ok_or_else(facade::unavailable)?;
        runtime
            .block_on(app.save_ui_display_options(self.display_options))
            .map_err(|error| facade::describe("Failed to save interface settings", &error))?;
        self.dirty = false;
        Ok(())
    }
}

/// Intents emitted by `render_interface_settings`. Caller translates
/// `Navigate` into a `SettingsAction::NavigateTo` and routes the
/// remaining variants through `handle_interface_settings_action`.
#[derive(Debug, Clone)]
pub enum InterfaceSettingsAction {
    /// First-render load: fetch the display options through the
    /// application and populate `state`. Auto-fired when `state.loaded`
    /// is false.
    LoadDisplayOptions,
    /// User mutated a display option (`state.dirty` is true). Save them
    /// through the application, then push the
    /// visible-effects-of-those-options into the `ui_preferences` signal
    /// and the active tab's `browser_view_state`.
    SaveDisplayOptions,
    /// User picked a layout target in the dialog — navigate to its
    /// editor page.
    Navigate(crate::core::navigation::SettingsPage),
    /// User toggled a single item's visibility in the context-menu or
    /// info-panel section. The dispatcher saves the item through the
    /// application and updates the corresponding signal.
    ToggleItemVisibility {
        region: UiRegionDto,
        item_id: String,
        visible: bool,
    },
}

/// Render the Interface settings page.
pub fn render_interface_settings(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    shared: &SharedState,
    interface_state: &mut InterfaceSettingsState,
) -> Option<InterfaceSettingsAction> {
    if !interface_state.loaded {
        let Some(error) = interface_state.error.clone() else {
            ui.label(
                egui::RichText::new("Loading interface settings…")
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
            );
            return Some(InterfaceSettingsAction::LoadDisplayOptions);
        };
        // The load is held, not abandoned: clearing the failure is what
        // lets the auto-fire above run again, so Retry needs no action of
        // its own.
        render_error(ui, theme, &error);
        ui.add_space(8.0);
        if ui
            .add(TextButton::new("Retry", ButtonSize::Small).with_theme_colors(&theme.colors))
            .clicked()
        {
            interface_state.error = None;
        }
        return None;
    }

    let mut emitted: Option<InterfaceSettingsAction> = None;
    // Set by whichever display-option control the user touched this
    // frame. Kept separate from `dirty` so a *new* edit is
    // distinguishable from one that is merely still unsaved -- only the
    // former clears a previous failure and re-arms the auto-save.
    let mut edited_this_frame = false;

    if let Some(error) = interface_state.error.clone() {
        render_error(ui, theme, &error);
        ui.add_space(8.0);
    }

    Form::new().id("interface_settings").show(ui, theme, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 16.0);

        // Toolbar section — just the Edit Layout button. Item
        // editing lives in the dedicated toolbar layout editor.
        render_section(ui, theme, "Toolbar", |ui| {
            ui.label(
                egui::RichText::new("Customize toolbar button arrangement")
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
            );
            ui.add_space(8.0);

            if ui
                .add(
                    TextButton::new(
                        format!("{} Edit Layout", egui_phosphor::regular::PENCIL_SIMPLE),
                        ButtonSize::Medium,
                    )
                    .with_theme_colors(&theme.colors),
                )
                .clicked()
            {
                interface_state.layout_dialog_open = true;
            }
        });

        // Context Menu section — visibility toggles backed by the
        // context_menu_items signal.
        render_section(ui, theme, "Context Menu", |ui| {
            let items = shared.signals().context_menu_items.get();
            if let Some((item_id, visible)) =
                sections::context_menu_section::render(ui, theme, &items)
            {
                capture_toggle(&mut emitted, UiRegionDto::ContextMenu, item_id, visible);
            }
        });

        // Info Panel section — visibility toggles backed by the
        // info_panel_items signal.
        render_section(ui, theme, "Info Panel", |ui| {
            let items = shared.signals().info_panel_items.get();
            if let Some((item_id, visible)) =
                sections::info_panel_section::render(ui, theme, &items)
            {
                capture_toggle(&mut emitted, UiRegionDto::InfoPanel, item_id, visible);
            }
        });

        // Layout section — display options.
        render_section(ui, theme, "Layout", |ui| {
            sections::layout_section::render(
                ui,
                theme,
                &mut interface_state.display_options,
                &mut edited_this_frame,
            );
        });

        // Header section — display option (show button labels).
        render_section(ui, theme, "Header", |ui| {
            ui.label(
                egui::RichText::new("Configure header button display")
                    .size(12.0)
                    .color(theme.colors.on_surface_variant),
            );
            ui.add_space(8.0);

            if ui
                .checkbox(
                    &mut interface_state.display_options.show_button_labels,
                    "Show button labels in header",
                )
                .on_hover_text("Display text labels next to icons in header buttons")
                .changed()
            {
                edited_this_frame = true;
            }
        });

        ui.add_space(16.0);
    });

    // Layout type selection dialog
    if interface_state.layout_dialog_open {
        egui::Window::new("Choose Layout to Edit")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.set_min_width(280.0);

                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("What would you like to customize?")
                            .size(14.0)
                            .color(theme.colors.on_surface),
                    );
                    ui.add_space(16.0);

                    ui.horizontal(|ui| {
                        if ui
                            .add_sized(
                                [120.0, 40.0],
                                egui::Button::new(format!(
                                    "{} Toolbar",
                                    egui_phosphor::regular::STACK
                                )),
                            )
                            .clicked()
                        {
                            interface_state.layout_dialog_open = false;
                            emitted = Some(InterfaceSettingsAction::Navigate(
                                crate::core::navigation::SettingsPage::ToolbarLayout,
                            ));
                        }

                        ui.add_space(16.0);

                        if ui
                            .add_sized(
                                [120.0, 40.0],
                                egui::Button::new(format!(
                                    "{} Info Panel",
                                    egui_phosphor::regular::SIDEBAR
                                )),
                            )
                            .clicked()
                        {
                            interface_state.layout_dialog_open = false;
                            emitted = Some(InterfaceSettingsAction::Navigate(
                                crate::core::navigation::SettingsPage::InfoPanelLayout,
                            ));
                        }
                    });

                    ui.add_space(12.0);

                    if ui
                        .add(
                            TextButton::new("Cancel", ButtonSize::Small)
                                .with_theme_colors(&theme.colors),
                        )
                        .clicked()
                    {
                        interface_state.layout_dialog_open = false;
                    }
                });
            });
    }

    // A fresh edit deserves a fresh attempt: it marks the page dirty and
    // clears any previous failure, which is what re-arms the auto-save
    // below.
    if edited_this_frame {
        interface_state.dirty = true;
        interface_state.error = None;
    }

    // Auto-save display options when dirty and no higher-priority
    // action is already in flight this frame. A page holding a failure
    // waits for the next edit instead of retrying every frame.
    if emitted.is_none() && interface_state.dirty && interface_state.error.is_none() {
        emitted = Some(InterfaceSettingsAction::SaveDisplayOptions);
    }

    emitted
}

/// The page's failure banner: a load that could not be served, or a save
/// the application refused.
fn render_error(ui: &mut egui::Ui, theme: &AppTheme, error: &str) {
    ui.label(
        egui::RichText::new(error)
            .size(12.0)
            .color(theme.colors.error),
    );
}

/// Helper to wrap a section's toggle event into a ToggleItemVisibility
/// action without clobbering a higher-priority emit (Navigate, etc.).
fn capture_toggle(
    emitted: &mut Option<InterfaceSettingsAction>,
    region: UiRegionDto,
    item_id: String,
    visible: bool,
) {
    if emitted.is_none() {
        *emitted = Some(InterfaceSettingsAction::ToggleItemVisibility {
            region,
            item_id,
            visible,
        });
    }
}

/// Dispatch an `InterfaceSettingsAction` against the application and
/// the shared signal graph. Called by the parent view after
/// `render_interface_settings` returns an action. All side effects on
/// stored state and on canonical signals live here.
pub fn handle_interface_settings_action(
    state: &mut InterfaceSettingsState,
    action: InterfaceSettingsAction,
    shared: &SharedState,
) {
    match action {
        InterfaceSettingsAction::Navigate(_) => {
            // Navigation is the caller's responsibility — it translates
            // to SettingsAction::NavigateTo and returns up the chain.
            // The dispatcher should never be called with this variant.
            debug_assert!(
                false,
                "InterfaceSettingsAction::Navigate should be handled by the caller, not the data dispatcher"
            );
        }
        InterfaceSettingsAction::LoadDisplayOptions => {
            match state.load(shared) {
                Ok(()) => state.error = None,
                Err(error) => {
                    tracing::warn!("{error}");
                    // Shown on the page, and stops the per-frame retry
                    // until the user acts again.
                    state.error = Some(error);
                }
            }
        }
        InterfaceSettingsAction::SaveDisplayOptions => {
            if let Err(error) = state.save(shared) {
                tracing::warn!("{error}");
                shared.toaster.lock().error(error.clone());
                state.error = Some(error);
                // The page keeps `dirty`, so nothing below may push a
                // preference the application refused to store.
                return;
            }
            state.error = None;

            // Push label preference into ui_preferences signal so the
            // header (which reads from it) repaints with new labels.
            let mut prefs = shared.signals().ui_preferences.get();
            prefs.show_button_labels = state.display_options.show_button_labels;
            shared.signals().ui_preferences.set(prefs);

            // Apply panel-visibility settings to the active tab's
            // browser_view_state so the change is visible immediately
            // on the currently-open tab.
            shared
                .signals()
                .tabs
                .get()
                .active()
                .browser_view_state
                .update(|s| {
                    s.toolbar_state.show_tree_panel = state.display_options.tree_panel_visible;
                    s.toolbar_state.show_properties_panel =
                        state.display_options.properties_panel_visible;
                });
        }
        InterfaceSettingsAction::ToggleItemVisibility {
            region,
            item_id,
            visible,
        } => {
            let Some((app, runtime)) = facade::handles(shared) else {
                tracing::warn!("{}", facade::unavailable());
                return;
            };

            let signal = match region {
                UiRegionDto::Toolbar => &shared.signals().toolbar_items,
                UiRegionDto::InfoPanel => &shared.signals().info_panel_items,
                UiRegionDto::ContextMenu => &shared.signals().context_menu_items,
                // No signal mirrors the tools dialog, and no section of
                // this page renders it either, so a toggle for it cannot
                // be emitted. Named rather than matched by wildcard so a
                // region added to the application forces this decision to
                // be revisited.
                UiRegionDto::ToolsDialog => {
                    debug_assert!(
                        false,
                        "no Interface section renders the tools dialog, so nothing can toggle it"
                    );
                    return;
                }
            };

            let mut items = signal.get();
            let Some(item) = items.iter_mut().find(|i| i.id == item_id) else {
                return;
            };
            item.visible = visible;
            let updated = item.clone();
            // Persist the single item. Upsert semantics mean naming one
            // item leaves the rest of the region alone.
            if let Err(error) = runtime.block_on(app.save_ui_items(region, vec![updated])) {
                tracing::warn!(
                    "{}",
                    facade::describe(
                        &format!("Failed to save the visibility of {item_id:?}"),
                        &error
                    )
                );
                return;
            }
            signal.set(items);
        }
    }
}

/// Helper function to render a settings section with consistent Y2K styling
fn render_section<R>(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    title: &str,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::NONE
        .stroke(egui::Stroke::new(1.0, theme.colors.outline))
        .corner_radius(egui::CornerRadius::ZERO)
        .inner_margin(spacing::CARD)
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(title)
                        .size(15.0)
                        .strong()
                        .color(theme.colors.on_surface),
                );
                ui.add_space(8.0);
                content(ui)
            })
            .inner
        })
        .inner
}
