//! Process page view — 3-panel layout: input | pipeline builder | preview+execute.

use super::state::ProcessPageState;
use super::step_widgets;
use crate::shared::SharedState;
use arclain_core::{
    CompressionLevel, ConvertFormat, PipelineInput, PipelineOutput, PipelineStep,
};
use arclain_widgets::{ButtonSize, TextButton};
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
    ui.heading("Input");
    ui.add_space(4.0);

    if ui
        .add(
            TextButton::new("Pick file(s)...", ButtonSize::Small)
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
            TextButton::new("Pick folder...", ButtonSize::Small)
                .with_theme_colors(&shared.theme.colors),
        )
        .clicked()
    {
        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
            state.pipeline.input = Some(PipelineInput::Folder(folder));
            state.mark_dirty();
        }
    }

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(4.0);

    match &state.pipeline.input {
        None => {
            ui.label(
                egui::RichText::new("No input selected")
                    .color(shared.theme.colors.on_surface_variant),
            );
        }
        Some(PipelineInput::Files(v)) => {
            ui.label(format!("{} file(s)", v.len()));
            egui::ScrollArea::vertical().show(ui, |ui| {
                for f in v {
                    ui.label(
                        egui::RichText::new(
                            f.file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("")
                                .to_string(),
                        )
                        .monospace()
                        .size(11.0),
                    );
                }
            });
        }
        Some(PipelineInput::Folder(p)) => {
            ui.label(format!(
                "Folder: {}",
                p.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
            ));
            ui.label(
                egui::RichText::new(p.to_string_lossy().to_string())
                    .size(10.0)
                    .color(shared.theme.colors.on_surface_variant),
            );
        }
    }
}

fn render_pipeline_panel(ui: &mut egui::Ui, shared: &SharedState, state: &mut ProcessPageState) {
    ui.heading("Pipeline");
    ui.add_space(4.0);

    // Load rules once per frame for the Organize step widget
    let rules: Vec<arclain_core::OrganizationRule> = shared
        .services
        .organization_service
        .as_ref()
        .and_then(|svc| svc.list_domain_rules().ok())
        .unwrap_or_default();

    let mut any_changed = false;

    ui.horizontal(|ui| {
        if ui.button("+ Flatten").clicked() {
            state.pipeline.steps.push(PipelineStep::Flatten {
                strip_common_prefix: true,
            });
            any_changed = true;
        }
        if ui.button("+ Organize").clicked() {
            state.pipeline.steps.push(PipelineStep::Organize { rule_id: 0 });
            any_changed = true;
        }
        if ui.button("+ Convert").clicked() {
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
                    ui.strong(format!("{}. {}", i + 1, step.display_name()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("✕").on_hover_text("Remove").clicked() {
                            remove_idx = Some(i);
                        }
                        if ui
                            .add_enabled(i + 1 < step_count, egui::Button::new("↓").small())
                            .on_hover_text("Move down")
                            .clicked()
                        {
                            move_down_idx = Some(i);
                        }
                        if ui
                            .add_enabled(i > 0, egui::Button::new("↑").small())
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
                    PipelineStep::Convert { .. } => step_widgets::render_convert_config(ui, step),
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
        ui.label(
            egui::RichText::new("Add a step to get started").color(shared.theme.colors.on_surface_variant),
        );
    }
}

fn render_preview_panel(ui: &mut egui::Ui, shared: &SharedState, state: &mut ProcessPageState) {
    ui.heading("Preview");
    ui.add_space(4.0);

    if state.preview.is_empty() && state.preview.global_warnings.is_empty() {
        ui.label(
            egui::RichText::new("Add input and operations to see preview")
                .color(shared.theme.colors.on_surface_variant),
        );
    } else {
        for w in &state.preview.global_warnings {
            ui.colored_label(shared.theme.colors.error, format!("⚠ {}", w));
        }

        ui.label(format!(
            "{} file(s) will be processed",
            state.preview.total_files()
        ));

        egui::ScrollArea::vertical()
            .id_salt("process_preview_scroll")
            .max_height(260.0)
            .show(ui, |ui| {
                for entry in &state.preview.entries {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(
                            entry
                                .input
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or(""),
                        )
                        .monospace()
                        .strong(),
                    );
                    for op in &entry.operations {
                        ui.label(format!("  → {}", op));
                    }
                    if let Some(out) = &entry.expected_output {
                        ui.label(
                            egui::RichText::new(format!(
                                "  ⇒ {}",
                                out.file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or_default()
                            ))
                            .color(shared.theme.colors.on_surface_variant)
                            .size(11.0),
                        );
                    }
                    for w in &entry.warnings {
                        ui.colored_label(shared.theme.colors.error, format!("  ⚠ {}", w));
                    }
                }
            });
    }

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);

    // Output picker
    ui.label("Output:");
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
    egui::ComboBox::from_id_salt("pipeline_output_picker")
        .selected_text(current_label)
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

    ui.add_space(12.0);

    let can_run = !state.preview.is_empty()
        && !state.pipeline.steps.is_empty()
        && !state.is_running;

    if ui
        .add_enabled(
            can_run,
            TextButton::new("Execute", ButtonSize::Medium)
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

    // Progress display while running
    let run_state = shared.signals().process_run.get();
    if run_state.is_running {
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
        ui.label(format!(
            "File {} of {}: {}",
            run_state.files_done + 1,
            run_state.files_total.max(1),
            run_state.current_file
        ));
        ui.label(format!("Step: {}", run_state.current_step));
        ui.add(
            egui::ProgressBar::new(run_state.step_percent as f32 / 100.0)
                .show_percentage(),
        );
        if run_state.files_failed > 0 {
            ui.colored_label(
                shared.theme.colors.error,
                format!("{} failed so far", run_state.files_failed),
            );
        }
    }

    if let Some(ref summary) = state.last_result_summary {
        ui.add_space(8.0);
        ui.label(summary);
    }
}
