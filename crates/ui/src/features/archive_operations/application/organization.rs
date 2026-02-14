use arclain_core::features::organization::ArchiveProfile;

/// Run organization plan asynchronously
///
/// If `profile` is provided, uses its compression settings.
/// Otherwise falls back to default 7z maximum compression.
pub fn run_organization_plan(
    shared: crate::shared::SharedState,
    plan: arclain_core::features::organization::engine::OrganizationPlan,
    source: std::path::PathBuf,
    dest: std::path::PathBuf,
    profile: Option<ArchiveProfile>,
) {
    let signals = shared.app_state.lock().signals.clone();

    // Set initial progress state
    signals
        .extraction_progress
        .set(Some(crate::core::signals::ExtractionProgressState {
            current_file: "Organizing archive...".to_string(),
            percent: 0,
            current: 0,
            total: 100,
            complete: false,
            error: None,
            file_to_open: None,
        }));

    // Spawn thread
    std::thread::spawn(move || {
        // We use the helper from features/organization/operations.rs which handles password retries
        let result =
            crate::features::organization::application::operations::execute_organization_plan(
                &shared, &plan, &source, &dest, profile.as_ref(),
            );

        match result {
            Ok(_) => {
                signals.extraction_progress.set(Some(
                    crate::core::signals::ExtractionProgressState {
                        current_file: "Organization completed".to_string(),
                        percent: 100,
                        current: 100,
                        total: 100,
                        complete: true,
                        error: None,
                        file_to_open: None, // Could set to dest to open?
                    },
                ));
            }
            Err(e) => {
                signals.extraction_progress.set(Some(
                    crate::core::signals::ExtractionProgressState {
                        current_file: "Organization failed".to_string(),
                        percent: 0,
                        current: 0,
                        total: 100,
                        complete: true,
                        error: Some(format!("{}", e)),
                        file_to_open: None,
                    },
                ));
            }
        }
    });
}
