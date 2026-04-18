//! Process page view — 3-panel layout: input | pipeline builder | preview+execute.

use super::state::ProcessPageState;
use super::step_widgets;
use crate::shared::SharedState;
use arclain_core::{
    CompressionLevel, ConvertFormat, OutputCollisionPolicy, PipelineInput, PipelineOutput,
    PipelineStep,
};
use arclain_widgets::{ButtonSize, IconButton, IconButtonSize, Text, TextButton, ThemedDropdown};
use eframe::egui;

pub fn render(ctx: &egui::Context, shared: &SharedState, state: &mut ProcessPageState) {
    state.refresh_preview();

    // Sync is_running from the signal
    let run_state = shared.signals().process_run.get();
    state.is_running = run_state.is_running;
    if run_state.completed && state.last_result_summary.as_deref() != run_state.summary.as_deref() {
        state.last_result_summary = run_state.summary.clone();
    }

    egui::TopBottomPanel::top("process_preset_bar").show(ctx, |ui| {
        ui.add_space(4.0);
        super::preset_bar::render(ui, shared, state);
        ui.add_space(4.0);
    });

    egui::SidePanel::left("process_input_panel")
        .resizable(true)
        .default_width(260.0)
        .show(ctx, |ui| render_input_panel(ui, shared, state));

    egui::SidePanel::right("process_preview_panel")
        .resizable(true)
        .default_width(340.0)
        .show(ctx, |ui| render_preview_panel(ui, shared, state));

    egui::CentralPanel::default().show(ctx, |ui| render_pipeline_panel(ui, shared, state));
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
                p.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
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

    // Load rules once per frame for the Organize step widget
    let rules: Vec<arclain_core::OrganizationRule> = shared
        .services
        .organization_service
        .as_ref()
        .and_then(|svc| svc.list_domain_rules().ok())
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
            state.pipeline.steps.push(PipelineStep::Organize { rule_id: 0 });
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
        state.refresh_preview();
    }

    if state.pipeline.steps.is_empty() {
        ui.add_space(12.0);
        Text::new("Add a step to get started").muted().show(ui);
    }
}

fn render_preview_panel(ui: &mut egui::Ui, shared: &SharedState, state: &mut ProcessPageState) {
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

        let header = format!(
            "{} file(s) will be processed",
            state.preview.total_files()
        );
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
                            out.file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or_default()
                        );
                        Text::new(&out_line).muted().size(11.0).show(ui);
                    }
                    for w in &entry.warnings {
                        let warn_line =
                            format!("  {} {}", egui_phosphor::regular::WARNING, w);
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
            p.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
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

    let can_run = !state.preview.is_empty()
        && !state.pipeline.steps.is_empty()
        && !state.is_running;

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
        crate::core::operations::process_runner::spawn_run(
            state.pipeline.clone(),
            shared.app_state.clone(),
            shared.services.clone(),
            shared.signals().process_run.clone(),
            &shared.services.tokio_runtime,
        );
    }

    if let Some(ref summary) = state.last_result_summary {
        ui.add_space(8.0);
        Text::new(summary).show(ui);
    }
}
