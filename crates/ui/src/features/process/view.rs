//! Process page view — 3-panel layout: input | pipeline builder | preview+execute.
//!
//! Architecture: render returns `Option<ProcessAction>` describing
//! intent — initial cache loads, preset persistence, or pipeline
//! execution. The sibling `handle_process_action` function owns all
//! application-facade calls and async spawning so the render path itself
//! stays a pure intent-emitter.
//!
//! Every action here goes through `ArclainApp`. The page no longer reads
//! the presets file, computes a preview, queries the configuration
//! database, or runs `execute_pipeline` on the shared runtime: it
//! assembles one `PipelinePreviewRequest`, previews it, and runs the
//! request that request converts into.

use super::state::{PipelineDraft, ProcessPageState};
use super::step_widgets;
use crate::shared::SharedState;
use arclain_app::operations::pipeline::{
    CompressionLevelDto, OutputArtifactDto, OutputCollisionPolicyDto, PipelineDestinationDto,
    PipelineInputsDto, PipelineStepDto,
};
use arclain_widgets::{ButtonSize, IconButton, IconButtonSize, Text, TextButton, ThemedDropdown};
use eframe::egui;

/// Intents emitted by `render`. Navigation-free; the dispatcher
/// (`handle_process_action`) owns all side effects.
#[derive(Debug, Clone)]
pub enum ProcessAction {
    /// Fetch the count of interrupted pipeline runs from the
    /// application. Fired once when `state.interrupted_run_count` is
    /// `None`.
    LoadInterruptedCount,
    /// Fetch organization rules from the application, cache them in
    /// `state.cached_org_rules`. Fired once per session when the cache
    /// is empty.
    LoadOrganizationRules,
    /// Fetch the saved pipeline presets. Fired once per session when
    /// `state.presets` is `None`.
    LoadPresets,
    /// Recompute the preview for the current draft through
    /// `ArclainApp::preview_pipeline`. Fired whenever the draft changed
    /// since the last preview.
    RefreshPreview,
    /// User clicked Execute — dispatch `ArclainApp::start_pipeline` for
    /// exactly the request the preview described.
    RunPipeline,
    /// Save the current draft as a preset under this name (the
    /// application upserts by name).
    SavePreset { name: String },
    /// Delete the named preset.
    DeletePreset { name: String },
}

/// The step-list header label for one step. Presentation text, so it
/// lives here rather than being asked of the request DTO.
fn step_title(step: &PipelineStepDto) -> &'static str {
    match step {
        PipelineStepDto::Flatten { .. } => "Flatten nested archives",
        PipelineStepDto::Organize { .. } => "Apply organization rule",
        PipelineStepDto::Convert { .. } => "Convert format",
    }
}

fn output_artifact_label(artifact: OutputArtifactDto) -> &'static str {
    match artifact {
        OutputArtifactDto::Archive => "Archive",
        OutputArtifactDto::Folder => "Folder",
    }
}

fn collision_policy_label(policy: OutputCollisionPolicyDto) -> &'static str {
    match policy {
        OutputCollisionPolicyDto::Fail => "Fail on existing",
        OutputCollisionPolicyDto::Skip => "Skip if exists",
        OutputCollisionPolicyDto::Overwrite => "Overwrite",
        OutputCollisionPolicyDto::Smart => "Smart (dedup / prompt)",
    }
}

pub fn render(
    ctx: &egui::Context,
    shared: &SharedState,
    state: &mut ProcessPageState,
) -> Option<ProcessAction> {
    let mut emitted: Option<ProcessAction> = None;

    // Auto-fire initial cache loads and the preview refresh. Each is
    // idempotent and guarded by its own "already have it" check, so
    // subsequent renders skip these branches. Only one fires per frame
    // and the rest follow on later frames — a few frames of warm-up is
    // imperceptible.
    if state.interrupted_run_count.is_none() {
        emitted = Some(ProcessAction::LoadInterruptedCount);
    } else if state.cached_org_rules.is_none() {
        emitted = Some(ProcessAction::LoadOrganizationRules);
    } else if state.presets.is_none() {
        emitted = Some(ProcessAction::LoadPresets);
    } else if state.preview_dirty {
        emitted = Some(ProcessAction::RefreshPreview);
    }

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
            let banner_text = format!(
                "{} {} pipeline run(s) were interrupted in a previous session.",
                egui_phosphor::regular::WARNING,
                state.interrupted_run_label()
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
                state.draft.inputs = PipelineInputsDto::Files { paths: files };
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
                // Kept as a folder rather than expanded here: the run
                // expands it, so an archive dropped into the folder
                // between now and Execute is still picked up.
                state.draft.inputs = PipelineInputsDto::Folder { path: folder };
                state.mark_dirty();
            }
        }
    });

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(4.0);

    match &state.draft.inputs {
        PipelineInputsDto::Files { paths } if paths.is_empty() => {
            Text::new("No input selected").muted().show(ui);
        }
        PipelineInputsDto::Files { paths } => {
            let count = format!("{} file(s)", paths.len());
            Text::new(&count).show(ui);
            egui::ScrollArea::vertical().show(ui, |ui| {
                for f in paths {
                    let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    Text::new(name).monospace().size(11.0).show(ui);
                }
            });
        }
        PipelineInputsDto::Folder { path } => {
            let folder_line = format!(
                "Folder: {}",
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
            );
            Text::new(&folder_line).show(ui);
            let full = path.to_string_lossy().into_owned();
            Text::new(&full).size(10.0).muted().show(ui);
        }
    }
}

fn render_pipeline_panel(ui: &mut egui::Ui, shared: &SharedState, state: &mut ProcessPageState) {
    Text::new("Pipeline").size(16.0).strong().show(ui);
    ui.add_space(6.0);

    // Snapshot the rules cache so we can iterate the draft's steps
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
            state.draft.steps.push(PipelineStepDto::Flatten {
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
            state.draft.steps.push(PipelineStepDto::Organize {
                rule_id: String::new(),
            });
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
            state.draft.steps.push(PipelineStepDto::Convert {
                format: "zip".to_string(),
                compression: CompressionLevelDto::Normal,
            });
            any_changed = true;
        }
    });

    ui.add_space(8.0);

    let mut remove_idx: Option<usize> = None;
    let mut move_up_idx: Option<usize> = None;
    let mut move_down_idx: Option<usize> = None;
    let step_count = state.draft.steps.len();

    for (i, step) in state.draft.steps.iter_mut().enumerate() {
        egui::Frame::NONE
            .fill(shared.theme.colors.surface_variant)
            .inner_margin(egui::Margin::same(8))
            .corner_radius(4.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let title = format!("{}. {}", i + 1, step_title(step));
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
                    PipelineStepDto::Flatten { .. } => {
                        step_widgets::render_flatten_config(ui, step)
                    }
                    PipelineStepDto::Convert { .. } => {
                        step_widgets::render_convert_config(ui, shared, step)
                    }
                    PipelineStepDto::Organize { .. } => {
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
        state.draft.steps.remove(i);
        any_changed = true;
    }
    if let Some(i) = move_up_idx {
        state.draft.steps.swap(i, i - 1);
        any_changed = true;
    }
    if let Some(i) = move_down_idx {
        state.draft.steps.swap(i, i + 1);
        any_changed = true;
    }

    if any_changed {
        // Marks only. The refresh itself is a dispatched action (see
        // `render`) — the render path never calls the application.
        state.mark_dirty();
    }

    if state.draft.steps.is_empty() {
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

    if state.preview.entries.is_empty() && state.preview.global_warnings.is_empty() {
        Text::new("Add input and operations to see preview")
            .muted()
            .show(ui);
    } else {
        for w in &state.preview.global_warnings {
            let line = format!("{} {}", egui_phosphor::regular::WARNING, w);
            Text::new(&line).color(shared.theme.colors.error).show(ui);
        }

        let header = format!("{} file(s) will be processed", state.preview.entries.len());
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
    let current = state.draft.destination.clone();
    let current_label = match &current {
        PipelineDestinationDto::SameFolder => "Same folder as input".to_string(),
        PipelineDestinationDto::Folder { path } => format!(
            "New folder: {}",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
        ),
    };
    ThemedDropdown::new("pipeline_output_picker", current_label)
        .with_theme_colors(&shared.theme.colors)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(
                    matches!(current, PipelineDestinationDto::SameFolder),
                    "Same folder as input",
                )
                .clicked()
            {
                state.draft.destination = PipelineDestinationDto::SameFolder;
                state.mark_dirty();
            }
            if ui.button("Pick folder...").clicked() {
                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    state.draft.destination = PipelineDestinationDto::Folder { path: folder };
                    state.mark_dirty();
                }
            }
        });

    ui.add_space(8.0);

    // Output artifact — produce an archive or leave as a folder.
    Text::new("Output as:").strong().show(ui);
    let current_artifact = state.draft.output_artifact;
    ThemedDropdown::new(
        "pipeline_output_artifact",
        output_artifact_label(current_artifact),
    )
    .with_theme_colors(&shared.theme.colors)
    .show_ui(ui, |ui| {
        for opt in [OutputArtifactDto::Archive, OutputArtifactDto::Folder] {
            if ui
                .selectable_label(current_artifact == opt, output_artifact_label(opt))
                .clicked()
            {
                state.draft.output_artifact = opt;
                state.mark_dirty();
            }
        }
    });

    ui.add_space(8.0);

    // Collision policy — controls what happens when output already exists.
    Text::new("If output exists:").strong().show(ui);
    let current_policy = state
        .draft
        .collision_policy
        .unwrap_or(OutputCollisionPolicyDto::Smart);
    ThemedDropdown::new(
        "pipeline_collision_policy",
        collision_policy_label(current_policy),
    )
    .with_theme_colors(&shared.theme.colors)
    .show_ui(ui, |ui| {
        for opt in [
            OutputCollisionPolicyDto::Smart,
            OutputCollisionPolicyDto::Skip,
            OutputCollisionPolicyDto::Overwrite,
            OutputCollisionPolicyDto::Fail,
        ] {
            if ui
                .selectable_label(current_policy == opt, collision_policy_label(opt))
                .clicked()
            {
                state.draft.collision_policy = Some(opt);
                state.mark_dirty();
            }
        }
    });

    ui.add_space(12.0);

    let can_run =
        !state.preview.entries.is_empty() && !state.draft.steps.is_empty() && !state.is_running;

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

/// Dispatch a `ProcessAction` against the application facade.
/// Called by the parent view (`core::arclain_app::content_handler`)
/// after `render` returns an action. Every facade call lives here, so
/// the render path stays a pure intent-emitter.
pub fn handle_process_action(
    state: &mut ProcessPageState,
    action: ProcessAction,
    shared: &SharedState,
) {
    let Some(app) = shared.facade.clone() else {
        tracing::error!("[process] no application facade available for {action:?}");
        // Seat the "already tried" markers so the render path does not
        // re-emit the same load action every frame against a facade that
        // is never going to appear (test fixtures build a facade-less
        // SharedState). Only ones still unset: a dispatch that cannot
        // answer must not overwrite an answer something else already
        // established.
        match action {
            ProcessAction::LoadInterruptedCount => {
                state.interrupted_run_count.get_or_insert(0);
            }
            ProcessAction::LoadOrganizationRules => {
                state.cached_org_rules.get_or_insert_with(Vec::new);
            }
            ProcessAction::LoadPresets => {
                state.presets.get_or_insert_with(Vec::new);
            }
            ProcessAction::RefreshPreview => state.preview_dirty = false,
            _ => {}
        }
        return;
    };
    let runtime = shared.services.tokio_runtime.clone();

    match action {
        ProcessAction::LoadInterruptedCount => {
            let count = runtime
                .block_on(app.interrupted_pipeline_runs(
                    // Everything ever recorded: nothing clears an
                    // interrupted run, and the page has no "since I last
                    // looked" timestamp of its own to pass instead.
                    0,
                    super::state::INTERRUPTED_RUN_QUERY_LIMIT,
                ))
                .unwrap_or_else(|error| {
                    tracing::warn!("[process] interrupted_pipeline_runs failed: {error:?}");
                    Vec::new()
                })
                .len();
            state.interrupted_run_count = Some(count);
        }
        ProcessAction::LoadOrganizationRules => {
            let rules = runtime
                .block_on(app.organization_rules())
                .unwrap_or_else(|error| {
                    tracing::warn!("[process] organization_rules failed: {error:?}");
                    Vec::new()
                });
            state.cached_org_rules = Some(rules);
        }
        ProcessAction::LoadPresets => {
            let presets = runtime
                .block_on(app.pipeline_presets())
                .unwrap_or_else(|error| {
                    tracing::warn!("[process] pipeline_presets failed: {error:?}");
                    Vec::new()
                });
            state.presets = Some(presets);
        }
        ProcessAction::RefreshPreview => {
            // Cleared first: a rejected preview (a step whose fields do
            // not parse — an Organize step with no rule chosen yet)
            // must not re-fire this action every frame.
            state.preview_dirty = false;
            match runtime.block_on(app.preview_pipeline(state.preview_request())) {
                Ok(preview) => state.preview = preview,
                Err(error) => {
                    // A half-built pipeline is previewable, so the only
                    // rejections here are steps that cannot run at all
                    // -- most often an Organize step whose rule has not
                    // been picked yet. Shown as the panel's own warning
                    // rather than blanking it: an empty preview would
                    // also disable Execute, leaving the user with a
                    // greyed-out button and no reason for it.
                    tracing::debug!("[process] preview_pipeline was rejected: {error:?}");
                    state.preview = arclain_app::process::PipelinePreviewDto {
                        entries: Vec::new(),
                        global_warnings: vec![error.summary.clone()],
                    };
                }
            }
        }
        ProcessAction::RunPipeline => {
            let origin_tab = shared.signals().tabs.get().active().clone();
            crate::core::operations::process_runner::start_pipeline_run(
                shared,
                // The very request the preview described — converted,
                // not rebuilt, so the run can never be a different
                // pipeline than the one on screen.
                state.preview_request(),
                origin_tab,
            );
        }
        ProcessAction::SavePreset { name } => {
            let draft = state.draft.clone();
            match runtime.block_on(app.save_pipeline_preset(preset_input(&name, &draft))) {
                Ok(presets) => {
                    state.presets = Some(presets);
                    state.active_preset_name = Some(name);
                }
                Err(error) => {
                    tracing::error!("[process] saving preset {name:?} failed: {error:?}");
                    shared.signals().status_bar.update(|status| {
                        status.message = format!("Could not save preset: {}", error.summary);
                    });
                }
            }
        }
        ProcessAction::DeletePreset { name } => {
            match runtime.block_on(app.delete_pipeline_preset(name.clone())) {
                Ok(presets) => {
                    state.presets = Some(presets);
                    if state.active_preset_name.as_deref() == Some(name.as_str()) {
                        state.active_preset_name = None;
                    }
                }
                Err(error) => {
                    tracing::error!("[process] deleting preset {name:?} failed: {error:?}");
                    shared.signals().status_bar.update(|status| {
                        status.message = format!("Could not delete preset: {}", error.summary);
                    });
                }
            }
        }
    }
}

/// The save request for `draft` under `name`.
///
/// A preset stores no input by design (see
/// `arclain_app::process::PipelinePresetSummary`), which is why the
/// draft's own `inputs` has no counterpart here.
fn preset_input(name: &str, draft: &PipelineDraft) -> arclain_app::process::PipelinePresetInput {
    arclain_app::process::PipelinePresetInput {
        name: name.to_string(),
        steps: draft.steps.clone(),
        destination: draft.destination.clone(),
        collision_policy: draft.collision_policy,
        output_artifact: draft.output_artifact,
    }
}
