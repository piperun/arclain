// Placeholder for organization operations that will be extracted from app/mod.rs
// This will contain the logic for executing organization plans, retrying with passwords, etc.

use crate::shared::SharedState;
use arclain_core::features::organization::engine::OrganizationPlan;
use arclain_core::features::organization::ArchiveProfile;
use std::path::PathBuf;
use tracing::{error, info};

pub fn execute_organization_plan(
    shared: &SharedState,
    plan: &OrganizationPlan,
    source: &PathBuf,
    dest: &PathBuf,
    profile: Option<&ArchiveProfile>,
) -> anyhow::Result<()> {
    let state = shared.app_state.lock();
    let backend_selector = state.backend_selector.clone();
    let temp_dir = state
        .user_config
        .temp_dir
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    drop(state);
    let password = shared.signals().current_password.get();

    info!(
        "Executing organization plan for archive: {}",
        source.display()
    );

    // Select appropriate backend based on archive type (RAR uses UnRAR, etc.)
    let backend = backend_selector.select(source)?;
    let archive = if let Some(ref pw) = password {
        info!(
            "Initializing archive handle with password (length: {})",
            pw.len()
        );
        arclain_core::Archive::with_password(backend, source, pw.clone())
    } else {
        info!("Initializing archive handle WITHOUT password");
        arclain_core::Archive::new(backend, source)
    };

    match arclain_core::features::organization::execute_organization_plan(
        &archive, dest, plan, &temp_dir, profile,
    ) {
        Ok(_) => Ok(()),
        Err(e) => {
            // If failed, try with auto-password if we didn't have one
            if password.is_none() {
                info!("Organization failed, trying with auto-password: {}", e);
                try_with_auto_password(shared, plan, source, dest, profile)
            } else {
                Err(e)
            }
        }
    }
}

pub fn try_with_auto_password(
    shared: &SharedState,
    plan: &OrganizationPlan,
    source: &PathBuf,
    dest: &PathBuf,
    profile: Option<&ArchiveProfile>,
) -> anyhow::Result<()> {
    let state = shared.app_state.lock();
    let backend_selector = state.backend_selector.clone();
    let temp_dir = state
        .user_config
        .temp_dir
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let archive_name = source.to_str();
    let pass_rules = state.pass_rules.clone();
    drop(state);

    let entries_arc = shared.signals().entries.get();
    let entries = entries_arc
        .iter()
        .map(|e| e.path.clone())
        .collect::<Vec<_>>();
    let detected_pw =
        arclain_core::utilities::auto_password_for(&pass_rules, archive_name, &entries);

    if let Some(ref password) = detected_pw {
        info!(
            "Retrying organization with auto-detected password (length: {})",
            password.len()
        );

        // Select appropriate backend based on archive type
        let backend = backend_selector.select(source)?;
        let archive_retry = arclain_core::Archive::with_password(backend, source, password.clone());

        match arclain_core::features::organization::execute_organization_plan(
            &archive_retry,
            dest,
            plan,
            &temp_dir,
            profile,
        ) {
            Ok(_) => {
                // Success! Save the password for future use
                shared
                    .signals()
                    .current_password
                    .set(Some(password.clone()));
                Ok(())
            }
            Err(e) => {
                error!("Organization retry with auto-password also failed: {}", e);
                Err(e)
            }
        }
    } else {
        info!("No password auto-detected from rules");
        Err(anyhow::anyhow!("No matching password rule found"))
    }
}
