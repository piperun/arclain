use arclain_app::error::ApplicationErrorKind;
use arclain_app::plugins::{PluginCapabilityDto, PluginInstallPreviewDto};
use arclain_ui::features::plugins::domain::types::{PluginsListState, RequestId};
use arclain_ui::features::plugins::presentation::views::{
    render_plugin_install_dialog, PluginInstallDialogResult,
};
use arclain_ui::shared::theme::AppTheme;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use egui_kittest::Harness;
use std::path::PathBuf;

#[derive(Default)]
struct DialogHarness {
    plugins: PluginsListState,
    emitted: Option<PluginInstallDialogResult>,
    theme: AppTheme,
}

fn preview() -> PluginInstallPreviewDto {
    PluginInstallPreviewDto {
        plugin_id: "tools.archive-viewer".to_string(),
        name: "Archive Viewer".to_string(),
        version: "1.4.2".to_string(),
        author: "Wirt Labs".to_string(),
        abi: "0.3.0".to_string(),
        capabilities: vec![
            PluginCapabilityDto::FileRead,
            PluginCapabilityDto::Network,
            PluginCapabilityDto::ArchiveMetadataRead,
        ],
        network_domains: vec![
            "metadata.example".to_string(),
            "updates.example".to_string(),
        ],
        fingerprint: "ab".repeat(32),
    }
}

fn ready_harness() -> Harness<'static, DialogHarness> {
    let mut plugins = PluginsListState::default();
    plugins.begin_package_inspection(PathBuf::from("archive-viewer.wirt"), RequestId(10));
    assert!(plugins.apply_package_preview(RequestId(10), preview()));
    Harness::new_ui_state(
        |ui, state| {
            let pending = state
                .plugins
                .pending_install
                .as_mut()
                .expect("pending install");
            if let Some(result) = render_plugin_install_dialog(ui.ctx(), &state.theme, pending) {
                state.emitted = Some(result);
            }
        },
        DialogHarness {
            plugins,
            ..Default::default()
        },
    )
}

#[test]
fn preview_dialog_shows_exact_identity_permissions_domains_and_fingerprint() {
    let mut harness = ready_harness();
    harness.run();

    for label in [
        "Review Wirt plugin",
        "Archive Viewer",
        "tools.archive-viewer",
        "Version 1.4.2",
        "Wirt Labs",
        "Wirt ABI 0.3.0",
        "FileRead",
        "Network",
        "ArchiveMetadataRead",
        "metadata.example",
        "updates.example",
        &"ab".repeat(32),
    ] {
        assert!(harness.query_by_label(label).is_some(), "missing {label:?}");
    }
    assert!(
        harness.query_by_label("FileWrite").is_none(),
        "the dialog must not invent unrequested permissions"
    );

    harness.get_by_label("Install").click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().emitted,
        Some(PluginInstallDialogResult::Install {
            package_path: PathBuf::from("archive-viewer.wirt"),
            expected_fingerprint: "ab".repeat(32),
        })
    );
}

#[test]
fn cancel_emits_no_install_and_an_active_install_cannot_be_dismissed() {
    let mut cancel = ready_harness();
    cancel.run();
    cancel.get_by_label("Cancel").click();
    cancel.run_steps(2);
    assert_eq!(
        cancel.state().emitted,
        Some(PluginInstallDialogResult::Cancel)
    );

    let mut installing = ready_harness();
    assert!(installing
        .state_mut()
        .plugins
        .begin_package_install(RequestId(11)));
    installing.run_steps(1);
    assert!(
        installing
            .get_by_label("Cancel")
            .accesskit_node()
            .is_disabled(),
        "an active install must not be dismissible"
    );
    assert!(
        installing
            .get_by_label("Installing…")
            .accesskit_node()
            .is_disabled(),
        "the install action must not dispatch twice"
    );
}

#[test]
fn stale_preview_and_failure_cannot_replace_a_newer_picker_result() {
    let mut plugins = PluginsListState::default();
    plugins.begin_package_inspection(PathBuf::from("old.wirt"), RequestId(20));
    plugins.begin_package_inspection(PathBuf::from("new.wirt"), RequestId(21));

    assert!(!plugins.apply_package_preview(RequestId(20), preview()));
    assert!(!plugins.apply_package_install_failure(
        RequestId(20),
        None,
        "old package failed".to_string()
    ));
    assert!(plugins.apply_package_preview(RequestId(21), preview()));

    let pending = plugins.pending_install.as_ref().expect("new picker result");
    assert_eq!(pending.package_path, PathBuf::from("new.wirt"));
    assert_eq!(
        pending.preview.as_ref().map(|value| value.name.as_str()),
        Some("Archive Viewer")
    );
    assert!(pending.error.is_none());
}

#[test]
fn domains_are_hidden_when_network_is_not_requested() {
    let mut plugins = PluginsListState::default();
    let mut local_preview = preview();
    local_preview.capabilities = vec![PluginCapabilityDto::FileRead];
    local_preview.network_domains = Vec::new();
    plugins.begin_package_inspection(PathBuf::from("local.wirt"), RequestId(30));
    assert!(plugins.apply_package_preview(RequestId(30), local_preview));

    let mut harness = Harness::new_ui_state(
        |ui, state: &mut DialogHarness| {
            let pending = state.plugins.pending_install.as_mut().unwrap();
            state.emitted = render_plugin_install_dialog(ui.ctx(), &state.theme, pending);
        },
        DialogHarness {
            plugins,
            ..Default::default()
        },
    );
    harness.run();

    assert!(harness.query_by_label("Network domains").is_none());
    assert!(harness.query_by_label("Install").is_some());
    assert!(harness.query_by_label("Cancel").is_some());
}

#[test]
fn facade_failure_classes_remain_distinguishable_in_the_dialog() {
    for (kind, heading) in [
        (ApplicationErrorKind::InvalidInput, "Invalid Wirt package"),
        (ApplicationErrorKind::Unsupported, "Unsupported Wirt ABI"),
        (ApplicationErrorKind::PermissionDenied, "Permission denied"),
        (ApplicationErrorKind::Conflict, "Plugin already installed"),
        (ApplicationErrorKind::Backend, "Plugin storage failure"),
    ] {
        let mut plugins = PluginsListState::default();
        plugins.begin_package_inspection(PathBuf::from("failure.wirt"), RequestId(40));
        assert!(plugins.apply_package_preview(RequestId(40), preview()));
        assert!(plugins.begin_package_install(RequestId(41)));
        assert!(plugins.apply_package_install_failure(
            RequestId(41),
            Some(kind),
            "plugin package could not be processed".to_string(),
        ));

        let mut harness = Harness::new_ui_state(
            |ui, state: &mut DialogHarness| {
                let pending = state.plugins.pending_install.as_mut().unwrap();
                state.emitted = render_plugin_install_dialog(ui.ctx(), &state.theme, pending);
            },
            DialogHarness {
                plugins,
                ..Default::default()
            },
        );
        harness.run();

        assert!(
            harness.query_by_label(heading).is_some(),
            "missing stable heading {heading:?}"
        );
        assert!(
            harness
                .query_by_label("plugin package could not be processed")
                .is_some(),
            "the bounded facade summary must remain visible"
        );
    }
}

#[test]
fn maximum_length_identity_fields_stay_inside_the_review_dialog() {
    let mut long_preview = preview();
    long_preview.author = "a".repeat(256);
    let author = long_preview.author.clone();
    let mut plugins = PluginsListState::default();
    plugins.begin_package_inspection(PathBuf::from("long-author.wirt"), RequestId(50));
    assert!(plugins.apply_package_preview(RequestId(50), long_preview));

    let mut harness = Harness::builder()
        .with_size(eframe::egui::vec2(800.0, 600.0))
        .build_ui_state(
            |ui, state: &mut DialogHarness| {
                let pending = state.plugins.pending_install.as_mut().unwrap();
                state.emitted = render_plugin_install_dialog(ui.ctx(), &state.theme, pending);
            },
            DialogHarness {
                plugins,
                ..Default::default()
            },
        );
    harness.run();

    let author_rect = harness.get_by_label(&author).rect();
    let abi_rect = harness.get_by_label("Wirt ABI 0.3.0").rect();
    assert!(
        author_rect.right() <= 660.0,
        "the full valid author must wrap within the 520 px modal: {author_rect:?}"
    );
    assert!(
        abi_rect.right() <= 660.0,
        "the ABI must remain visible after a maximum-length author: {abi_rect:?}"
    );
}
