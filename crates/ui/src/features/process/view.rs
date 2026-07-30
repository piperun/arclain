//! Process page view — 3-panel layout: input | pipeline builder | preview+execute.
//!
//! Architecture: render returns `Option<ProcessAction>` describing
//! intent — initial cache loads, preset persistence, or pipeline
//! execution. The sibling `handle_process_action` function owns all
//! DB / file-IO / async-spawn side effects so the render path itself
//! stays a pure intent-emitter.

use super::state::ProcessPageState;
use super::step_widgets;
use crate::shared::SharedState;
use arclain_core::{
    CompressionLevel, ConvertFormat, OutputArtifact, OutputCollisionPolicy, PipelineInput,
    PipelineOutput, PipelineStep,
};
use arclain_widgets::{ButtonSize, IconButton, IconButtonSize, Text, TextButton, ThemedDropdown};
use eframe::egui;

/// Intents emitted by `render`. Navigation-free; the dispatcher
/// (`handle_process_action`) owns all side effects.
#[derive(Debug, Clone)]
pub enum ProcessAction {
    /// Fetch the count of interrupted pipeline runs from the config DB.
    /// Fired once when `state.interrupted_run_count` is `None`.
    LoadInterruptedCount,
    /// Fetch organization rules from the service, cache them in
    /// `state.cached_org_rules`. Fired once per session when the cache
    /// is empty.
    LoadOrganizationRules,
    /// User clicked Execute — spawn the pipeline run on the tokio
    /// runtime via `core::operations::process_runner::spawn_run`.
    RunPipeline,
    /// Presets list mutated (saved or deleted) — persist to disk.
    SavePresets,
}

/// The active tab's product metadata, in the shape the core pipeline
/// preview still takes. TRANSITIONAL, and owned by the pipeline-preview
/// migration rather than by this page -- see
/// [`crate::core::tabs::legacy_metadata`].
fn selected_pipeline_metadata(shared: &SharedState) -> Option<arclain_core::GameMetadata> {
    shared
        .signals()
        .tabs
        .get()
        .active()
        .game_metadata
        .get()
        .as_ref()
        .map(crate::core::tabs::legacy_pipeline_metadata)
}

pub fn render(
    ctx: &egui::Context,
    shared: &SharedState,
    state: &mut ProcessPageState,
) -> Option<ProcessAction> {
    let mut emitted: Option<ProcessAction> = None;

    // Auto-fire initial cache loads. Both are session-cached after
    // first dispatch; subsequent renders skip these branches. If both
    // need loading, only one fires this frame and the other fires
    // next frame — 2-frame warm-up is imperceptible.
    if state.interrupted_run_count.is_none() {
        emitted = Some(ProcessAction::LoadInterruptedCount);
    } else if state.cached_org_rules.is_none() {
        emitted = Some(ProcessAction::LoadOrganizationRules);
    }

    let selected_metadata = selected_pipeline_metadata(shared);
    state.refresh_preview(selected_metadata.as_ref());

    // Sync is_running from the signal
    let run_state = shared.signals().process_run.get();
    state.is_running = run_state.is_running;
    if run_state.completed && state.last_result_summary.as_deref() != run_state.summary.as_deref() {
        state.last_result_summary = run_state.summary.clone();
    }

    // Non-blocking banner: previous runs interrupted (process killed mid-pipeline).
    let show_interrupted_banner = !state.interrupted_banner_dismissed
        && state.interrupted_run_count.map(|n| n > 0).unwrap_or(false);
    if show_interrupted_banner {
        egui::TopBottomPanel::top("process_interrupted_banner").show(ctx, |ui| {
            ui.add_space(4.0);
            let count = state.interrupted_run_count.unwrap_or(0);
            let banner_text = format!(
                "{} {} pipeline run(s) were interrupted in a previous session.",
                egui_phosphor::regular::WARNING,
                count
            );
            ui.horizontal(|ui| {
                Text::new(&banner_text)
                    .color(shared.theme.colors.error)
                    .show(ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            TextButton::new("Dismiss", ButtonSize::Small)
                                .with_theme_colors(&shared.theme.colors),
                        )
                        .clicked()
                    {
                        state.interrupted_banner_dismissed = true;
                    }
                });
            });
            ui.add_space(4.0);
        });
    }

    egui::TopBottomPanel::top("process_preset_bar").show(ctx, |ui| {
        ui.add_space(4.0);
        if let Some(action) = super::preset_bar::render(ui, shared, state) {
            if emitted.is_none() {
                emitted = Some(action);
            }
        }
        ui.add_space(4.0);
    });

    egui::SidePanel::left("process_input_panel")
        .resizable(true)
        .default_width(260.0)
        .show(ctx, |ui| render_input_panel(ui, shared, state));

    egui::SidePanel::right("process_preview_panel")
        .resizable(true)
        .default_width(340.0)
        .show(ctx, |ui| {
            if let Some(action) = render_preview_panel(ui, shared, state) {
                if emitted.is_none() {
                    emitted = Some(action);
                }
            }
        });

    egui::CentralPanel::default().show(ctx, |ui| render_pipeline_panel(ui, shared, state));

    emitted
}

fn render_input_panel(ui: &mut egui::Ui, shared: &SharedState, state: &mut ProcessPageState) {
    Text::new("Input").size(16.0).strong().show(ui);
    ui.add_space(6.0);

    ui.horizontal_wrapped(|ui| {
        if ui
            .add(
                TextButton::new(
                    format!("{} Files", egui_phosphor::regular::FILE),
                    ButtonSize::Small,
                )
                .with_theme_colors(&shared.theme.colors),
            )
            .clicked()
        {
            if let Some(files) = rfd::FileDialog::new()
                .add_filter("Archives", &["rar", "zip", "7z"])
                .pick_files()
            {
                state.pipeline.input = Some(PipelineInput::Files(files));
                state.mark_dirty();
            }
        }
        if ui
            .add(
                TextButton::new(
                    format!("{} Folder", egui_phosphor::regular::FOLDER),
                    ButtonSize::Small,
                )
                .with_theme_colors(&shared.theme.colors),
            )
            .clicked()
        {
            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                state.pipeline.input = Some(PipelineInput::Folder(folder));
                state.mark_dirty();
            }
        }
    });

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(4.0);

    match &state.pipeline.input {
        None => {
            Text::new("No input selected").muted().show(ui);
        }
        Some(PipelineInput::Files(v)) => {
            let count = format!("{} file(s)", v.len());
            Text::new(&count).show(ui);
            egui::ScrollArea::vertical().show(ui, |ui| {
                for f in v {
                    let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    Text::new(name).monospace().size(11.0).show(ui);
                }
            });
        }
        Some(PipelineInput::Folder(p)) => {
            let folder_line = format!(
                "Folder: {}",
                p.file_name().and_then(|n| n.to_str()).unwrap_or_default()
            );
            Text::new(&folder_line).show(ui);
            let full = p.to_string_lossy().into_owned();
            Text::new(&full).size(10.0).muted().show(ui);
        }
    }
}

fn render_pipeline_panel(ui: &mut egui::Ui, shared: &SharedState, state: &mut ProcessPageState) {
    Text::new("Pipeline").size(16.0).strong().show(ui);
    ui.add_space(6.0);

    // Snapshot the rules cache so we can iterate state.pipeline.steps
    // mutably without holding a borrow on state.cached_org_rules. The
    // cache is populated by the LoadOrganizationRules dispatch (auto-
    // fired from `render` when the cache is empty). Empty slice until
    // the dispatcher runs.
    let rules: Vec<arclain_app::organization::OrganizationRuleSummary> = state
        .cached_org_rules
        .as_deref()
        .map(|v| v.to_vec())
        .unwrap_or_default();

    let mut any_changed = false;

    ui.horizontal_wrapped(|ui| {
        if ui
            .add(
                TextButton::new(
                    format!("{} Flatten", egui_phosphor::regular::PLUS),
                    ButtonSize::Small,
                )
                .with_theme_colors(&shared.theme.colors),
            )
            .clicked()
        {
            state.pipeline.steps.push(PipelineStep::Flatten {
                strip_common_prefix: true,
                max_depth: 1,
            });
            any_changed = true;
        }
        if ui
            .add(
                TextButton::new(
                    format!("{} Organize", egui_phosphor::regular::PLUS),
                    ButtonSize::Small,
                )
                .with_theme_colors(&shared.theme.colors),
            )
            .clicked()
        {
            state
                .pipeline
                .steps
                .push(PipelineStep::Organize { rule_id: 0 });
            any_changed = true;
        }
        if ui
            .add(
                TextButton::new(
                    format!("{} Convert", egui_phosphor::regular::PLUS),
                    ButtonSize::Small,
                )
                .with_theme_colors(&shared.theme.colors),
            )
            .clicked()
        {
            state.pipeline.steps.push(PipelineStep::Convert {
                format: ConvertFormat::Zip,
                compression: CompressionLevel::Normal,
                password: None,
            });
            any_changed = true;
        }
    });

    ui.add_space(8.0);

    let mut remove_idx: Option<usize> = None;
    let mut move_up_idx: Option<usize> = None;
    let mut move_down_idx: Option<usize> = None;
    let step_count = state.pipeline.steps.len();

    for (i, step) in state.pipeline.steps.iter_mut().enumerate() {
        egui::Frame::NONE
            .fill(shared.theme.colors.surface_variant)
            .inner_margin(egui::Margin::same(8))
            .corner_radius(4.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let title = format!("{}. {}", i + 1, step.display_name());
                    Text::new(&title).strong().show(ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                IconButton::new(egui_phosphor::regular::X)
                                    .size(IconButtonSize::Small)
                                    .with_theme_colors(&shared.theme.colors),
                            )
                            .on_hover_text("Remove")
                            .clicked()
                        {
                            remove_idx = Some(i);
                        }
                        if ui
                            .add(
                                IconButton::new(egui_phosphor::regular::ARROW_DOWN)
                                    .size(IconButtonSize::Small)
                                    .enabled(i + 1 < step_count)
                                    .with_theme_colors(&shared.theme.colors),
                            )
                            .on_hover_text("Move down")
                            .clicked()
                        {
                            move_down_idx = Some(i);
                        }
                        if ui
                            .add(
                                IconButton::new(egui_phosphor::regular::ARROW_UP)
                                    .size(IconButtonSize::Small)
                                    .enabled(i > 0)
                                    .with_theme_colors(&shared.theme.colors),
                            )
                            .on_hover_text("Move up")
                            .clicked()
                        {
                            move_up_idx = Some(i);
                        }
                    });
                });
                ui.add_space(4.0);

                let changed = match step {
                    PipelineStep::Flatten { .. } => step_widgets::render_flatten_config(ui, step),
                    PipelineStep::Convert { .. } => {
                        step_widgets::render_convert_config(ui, shared, step)
                    }
                    PipelineStep::Organize { .. } => {
                        step_widgets::render_organize_config(ui, step, &rules)
                    }
                };
                if changed {
                    any_changed = true;
                }
            });
        ui.add_space(4.0);
    }

    if let Some(i) = remove_idx {
        state.pipeline.steps.remove(i);
        any_changed = true;
    }
    if let Some(i) = move_up_idx {
        state.pipeline.steps.swap(i, i - 1);
        any_changed = true;
    }
    if let Some(i) = move_down_idx {
        state.pipeline.steps.swap(i, i + 1);
        any_changed = true;
    }

    if any_changed {
        state.mark_dirty();
        let selected_metadata = selected_pipeline_metadata(shared);
        state.refresh_preview(selected_metadata.as_ref());
    }

    if state.pipeline.steps.is_empty() {
        ui.add_space(12.0);
        Text::new("Add a step to get started").muted().show(ui);
    }
}

fn render_preview_panel(
    ui: &mut egui::Ui,
    shared: &SharedState,
    state: &mut ProcessPageState,
) -> Option<ProcessAction> {
    let mut emitted: Option<ProcessAction> = None;
    Text::new("Preview").size(16.0).strong().show(ui);
    ui.add_space(6.0);

    if state.preview.is_empty() && state.preview.global_warnings.is_empty() {
        Text::new("Add input and operations to see preview")
            .muted()
            .show(ui);
    } else {
        for w in &state.preview.global_warnings {
            let line = format!("{} {}", egui_phosphor::regular::WARNING, w);
            Text::new(&line).color(shared.theme.colors.error).show(ui);
        }

        let header = format!("{} file(s) will be processed", state.preview.total_files());
        Text::new(&header).show(ui);

        egui::ScrollArea::vertical()
            .id_salt("process_preview_scroll")
            .max_height(260.0)
            .show(ui, |ui| {
                for entry in &state.preview.entries {
                    ui.add_space(4.0);
                    Text::new(
                        entry
                            .input
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or(""),
                    )
                    .monospace()
                    .strong()
                    .show(ui);
                    for op in &entry.operations {
                        let op_line = format!("  → {}", op);
                        Text::new(&op_line).show(ui);
                    }
                    if let Some(out) = &entry.expected_output {
                        let out_line = format!(
                            "  ⇒ {}",
                            out.file_name().and_then(|n| n.to_str()).unwrap_or_default()
                        );
                        Text::new(&out_line).muted().size(11.0).show(ui);
                    }
                    for w in &entry.warnings {
                        let warn_line = format!("  {} {}", egui_phosphor::regular::WARNING, w);
                        Text::new(&warn_line)
                            .color(shared.theme.colors.error)
                            .show(ui);
                    }
                }
            });
    }

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);

    // Output picker
    Text::new("Output:").strong().show(ui);
    let current = state.pipeline.output.clone();
    let current_label = match &current {
        PipelineOutput::SameFolder => "Same folder as input".to_string(),
        PipelineOutput::NewFolder(p) => format!(
            "New folder: {}",
            p.file_name().and_then(|n| n.to_str()).unwrap_or_default()
        ),
    };
    ThemedDropdown::new("pipeline_output_picker", current_label)
        .with_theme_colors(&shared.theme.colors)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(
                    matches!(current, PipelineOutput::SameFolder),
                    "Same folder as input",
                )
                .clicked()
            {
                state.pipeline.output = PipelineOutput::SameFolder;
                state.mark_dirty();
            }
            if ui.button("Pick folder...").clicked() {
                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    state.pipeline.output = PipelineOutput::NewFolder(folder);
                    state.mark_dirty();
                }
            }
        });

    ui.add_space(8.0);

    // Output artifact — produce an archive or leave as a folder.
    Text::new("Output as:").strong().show(ui);
    let current_artifact = state.pipeline.output_artifact;
    ThemedDropdown::new("pipeline_output_artifact", current_artifact.display_name())
        .with_theme_colors(&shared.theme.colors)
        .show_ui(ui, |ui| {
            for opt in [OutputArtifact::Archive, OutputArtifact::Folder] {
                if ui
                    .selectable_label(current_artifact == opt, opt.display_name())
                    .clicked()
                {
                    state.pipeline.output_artifact = opt;
                    state.mark_dirty();
                }
            }
        });

    ui.add_space(8.0);

    // Collision policy — controls what happens when output already exists.
    Text::new("If output exists:").strong().show(ui);
    let current_policy = state
        .pipeline
        .collision_policy
        .unwrap_or(OutputCollisionPolicy::Smart);
    ThemedDropdown::new("pipeline_collision_policy", current_policy.display_name())
        .with_theme_colors(&shared.theme.colors)
        .show_ui(ui, |ui| {
            for opt in [
                OutputCollisionPolicy::Smart,
                OutputCollisionPolicy::Skip,
                OutputCollisionPolicy::Overwrite,
                OutputCollisionPolicy::Fail,
            ] {
                if ui
                    .selectable_label(current_policy == opt, opt.display_name())
                    .clicked()
                {
                    state.pipeline.collision_policy = Some(opt);
                    state.mark_dirty();
                }
            }
        });

    ui.add_space(12.0);

    let can_run =
        !state.preview.is_empty() && !state.pipeline.steps.is_empty() && !state.is_running;

    if ui
        .add_enabled(
            can_run,
            TextButton::new(
                format!("{} Execute", egui_phosphor::regular::PLAY),
                ButtonSize::Medium,
            )
            .with_theme_colors(&shared.theme.colors),
        )
        .clicked()
    {
        emitted = Some(ProcessAction::RunPipeline);
    }

    if let Some(ref summary) = state.last_result_summary {
        ui.add_space(8.0);
        Text::new(summary).show(ui);
    }

    emitted
}

/// Dispatch a `ProcessAction` against the shared services / runtime.
/// Called by the parent view (`core::arclain_app::content_handler`)
/// after `render` returns an action. All side effects on the DB,
/// filesystem, and tokio runtime live here, so the render path stays
/// a pure intent-emitter.
pub fn handle_process_action(
    state: &mut ProcessPageState,
    action: ProcessAction,
    shared: &SharedState,
) {
    match action {
        ProcessAction::LoadInterruptedCount => {
            state.ensure_interrupted_count(shared.services.config_db.as_ref());
        }
        ProcessAction::LoadOrganizationRules => {
            let rules = shared
                .facade
                .as_ref()
                .and_then(|app| {
                    shared
                        .services
                        .tokio_runtime
                        .block_on(app.organization_rules())
                        .ok()
                })
                .unwrap_or_default();
            state.cached_org_rules = Some(rules);
        }
        ProcessAction::RunPipeline => {
            let origin_tab = shared.signals().tabs.get().active().clone();
            crate::core::operations::process_runner::spawn_run(
                state.pipeline.clone(),
                shared.app_state.clone(),
                shared.services.clone(),
                shared.signals().process_run.clone(),
                &shared.services.tokio_runtime,
                origin_tab,
            );
        }
        ProcessAction::SavePresets => {
            state.save_presets();
        }
    }
}
