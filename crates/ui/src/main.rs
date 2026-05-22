// Hide console window on Windows release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::Result;
use arclain_core::utilities::init_logging;
use arclain_ui::core::arclain_app::ArclainApp;
use eframe::egui;
use tracing::info;

// Force High Performance GPU on Windows
#[cfg(target_os = "windows")]
#[no_mangle]
pub static NvOptimusEnablement: u32 = 1;

#[cfg(target_os = "windows")]
#[no_mangle]
pub static AmdPowerXpressRequestHighPerformance: i32 = 1;

fn main() -> Result<()> {
    if let Err(e) = init_logging() {
        eprintln!("Failed to initialize logging: {}", e);
    }

    info!("Starting Arclain application");

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 800.0])
        .with_title("Arclain - Archive Viewer")
        .with_visible(true)
        .with_resizable(true)
        .with_decorations(true)
        .with_drag_and_drop(true);

    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
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

/// Rasterize the bundled SVG app icon to RGBA8 at 256×256 for use as
/// the OS window/taskbar icon. The SVG is `include_bytes!`'d so the
/// binary is self-contained — no runtime filesystem lookup.
///
/// Returns `None` if any step fails (malformed SVG, zero-size pixmap,
/// etc.). The caller treats absence as "no custom icon" rather than a
/// fatal error so a broken icon never blocks startup.
fn load_app_icon() -> Option<egui::IconData> {
    let svg_bytes = include_bytes!("../../../assets/icon.svg");
    let tree = resvg::usvg::Tree::from_data(svg_bytes, &resvg::usvg::Options::default()).ok()?;

    let size = tree.size().to_int_size();
    let scale = 256.0 / size.width().max(size.height()) as f32;
    let target_w = (size.width() as f32 * scale).round() as u32;
    let target_h = (size.height() as f32 * scale).round() as u32;

    let mut pixmap = resvg::tiny_skia::Pixmap::new(target_w, target_h)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    Some(egui::IconData {
        rgba: pixmap.data().to_vec(),
        width: target_w,
        height: target_h,
    })
}
