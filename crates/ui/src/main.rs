// Hide console window on Windows release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::Result;
use arclain_core::utilities::init_logging;
use arclain_ui::core::arclain_app::ArclainApp;
use eframe::egui;
use tracing::info;

fn main() -> Result<()> {
    if let Err(e) = init_logging() {
        eprintln!("Failed to initialize logging: {}", e);
    }

    info!("Starting Arclain application");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title("Arclain - Archive Viewer")
            .with_visible(true)
            .with_resizable(true)
            .with_decorations(true)
            .with_drag_and_drop(true),
        ..Default::default()
    };

    // Use new app structure
    eframe::run_native(
        "Arclain",
        options,
        Box::new(|cc| Ok(Box::new(ArclainApp::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("Failed to run app: {}", e))?;

    info!("Application shutting down");
    Ok(())
}
