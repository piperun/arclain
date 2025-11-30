use crate::app::state::AppState;
use crate::features::status_bar::StatusBarInfo;
use arclain_core::archive_organizer::organize_archive;
use parking_lot::Mutex;
use std::sync::Arc;
use tracing::info;

/// Organize archive with game metadata
pub fn organize_with_metadata(
    state: &Arc<Mutex<AppState>>,
    status_info: &mut StatusBarInfo,
) -> anyhow::Result<()> {
    let (source_path, metadata, temp_dir) = {
        let state_lock = state.lock();

        // Get current archive
        let source = state_lock
            .current_archive
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No archive loaded"))?
            .clone();

        // Get metadata
        let metadata = state_lock
            .current_game_metadata
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No metadata available"))?
            .clone();

        // Get temp directory from config
        let temp_dir = state_lock
            .cfg
            .cfg
            .temp_dir
            .clone()
            .unwrap_or_else(|| std::env::temp_dir());

        (source, metadata, temp_dir)
    };

    info!("Organizing archive: {}", source_path.display());
    status_info.message = "Organizing archive...".to_string();

    // Create destination path (same directory, with product_id as name)
    let dest_name = format!("{}.7z", metadata.product_id);
    let dest_path = source_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine parent directory"))?
        .join(&dest_name);

    // Get backend
    let backend = {
        let state_lock = state.lock();
        state_lock.backend.clone()
    };

    // Perform organization
    organize_archive(&backend, &source_path, &dest_path, &metadata, &temp_dir)?;

    info!("Archive organized successfully: {}", dest_path.display());
    status_info.message = format!("Organized as {}", dest_name);

    Ok(())
}
