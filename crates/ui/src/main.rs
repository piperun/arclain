mod core;
mod features;
mod platform;
mod shared;

use anyhow::Result;
use arclain_core::utilities::init_logging;
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
            .with_decorations(true),
        ..Default::default()
    };

    // Use new app structure
    eframe::run_native(
        "Arclain",
        options,
        Box::new(|cc| Ok(Box::new(core::arclain_app::ArclainApp::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("Failed to run app: {}", e))?;

    info!("Application shutting down");
    Ok(())
}
