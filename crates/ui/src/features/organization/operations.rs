// Placeholder for organization operations that will be extracted from app/mod.rs
// This will contain the logic for executing organization plans, retrying with passwords, etc.

use crate::shared::SharedState;
use arclain_core::features::organization::engine::OrganizationPlan;
use std::path::PathBuf;
use tracing::{error, info};

pub fn execute_organization_plan(
    shared: &SharedState,
    plan: &OrganizationPlan,
    source: &PathBuf,
    dest: &PathBuf,
) -> anyhow::Result<()> {
    let state = shared.app_state.lock();
    let backend_selector = state.backend_selector.clone();
    let temp_dir = state
        .cfg
        .cfg
        .temp_dir
        .clone()
        .unwrap_or_else(std::env::temp_dir);
    let password = state.current_password.clone();
    drop(state);

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
        &archive, dest, plan, &temp_dir,
    ) {
        Ok(_) => Ok(()),
        Err(e) => {
            // If failed, try with auto-password if we didn't have one
            if password.is_none() {
                info!("Organization failed, trying with auto-password: {}", e);
                try_with_auto_password(shared, plan, source, dest)
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
) -> anyhow::Result<()> {
    let state = shared.app_state.lock();
    let backend_selector = state.backend_selector.clone();
    let temp_dir = state
        .cfg
        .cfg
        .temp_dir
        .clone()
        .unwrap_or_else(std::env::temp_dir);
    let archive_name = source.to_str();
    let entries = state
        .all_entries
        .iter()
        .map(|e| e.path.clone())
        .collect::<Vec<_>>();
    let detected_pw = state.cfg.auto_password_for(archive_name, &entries);
    drop(state);

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
        ) {
            Ok(_) => {
                // Success! Save the password for future use
                let mut state = shared.app_state.lock();
                state.current_password = Some(password.clone());
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
