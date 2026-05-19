pub mod file_list;
// Moved from `shared/components/` in the 2026-05-19 audit cleanup —
// these components are archive_browser-specific (they render archive
// properties + plugin extensions), and keeping them in `shared/`
// dragged `features/plugins` imports into the shared layer.
pub mod panel;
pub mod properties_panel;
