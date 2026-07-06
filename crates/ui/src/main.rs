// Hide console window on Windows release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::Result;
use arclain_core::utilities::{current_app_log_path, init_logging, plugin_log_dir};
use arclain_ui::core::arclain_app::ArclainApp;
use arclain_ui::shared::components::logs_page::LogSession;
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

    let app_log_path = current_app_log_path();
    let app_log_offset = std::fs::metadata(&app_log_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let log_session =
        LogSession::capture_with_app_offset(app_log_path, app_log_offset, plugin_log_dir());

    info!("Starting Arclain application");

    // ── Wayland → XWayland fallback for drag-and-drop ──────────────────
    //
    // winit 0.30.x doesn't implement Wayland drag-and-drop — `egui`'s
    // `dropped_files` / `hovered_files` never fire on native Wayland.
    // The bug is tracked upstream at winit #1881 (5+ years open) and
    // egui #1563. PR #4571 (Slint team) reworks the DnD API but has
    // Wayland explicitly stubbed; Wayland implementation is a follow-up.
    //
    // Until that follow-up lands and propagates through to egui-winit,
    // we force the X11 backend on Wayland sessions by clearing
    // `WAYLAND_DISPLAY` before eframe init. winit then connects to the
    // X11 socket (XWayland on Wayland systems) and DnD works through
    // the XDND protocol that winit *does* fully support.
    //
    // Trade-off: gives up Wayland-native fractional scaling and a few
    // protocol niceties. Both are mostly invisible at our render
    // resolutions; broken DnD is not. Set `ARCLAIN_FORCE_WAYLAND=1` to
    // opt back into native Wayland if you don't need DnD.
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("ARCLAIN_FORCE_WAYLAND").is_none()
            && std::env::var_os("WAYLAND_DISPLAY").is_some()
        {
            info!(
                "Wayland session detected — forcing XWayland for drag-and-drop \
                 (see winit #1881). Set ARCLAIN_FORCE_WAYLAND=1 to opt back \
                 into native Wayland."
            );
            // SAFETY: we're still single-threaded at this point —
            // eframe::run_native below is what spawns the event loop
            // and any worker threads. Removing an env var is sound
            // when no other thread can read it concurrently.
            unsafe {
                std::env::remove_var("WAYLAND_DISPLAY");
            }
        }
    }

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
        Box::new(move |cc| {
            Ok(Box::new(ArclainApp::new_with_log_session(
                cc,
                log_session.clone(),
            )))
        }),
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
