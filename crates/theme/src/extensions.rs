//! Theme extensions for optional helper colors
//!
//! These are secondary colors that can be customized by users/plugins
//! but have sensible defaults derived from the core theme.

use egui::Color32;

use crate::ThemeColors;

/// Extension colors for specific use cases (file types, status badges, etc.)
///
/// These derive from core theme colors by default but can be overridden.
/// File-type defaults are the Tailwind palette values that the file
/// list grid view was already using inline; ThemeExtensions just gives
/// them a single home so the mapping can be themed.
#[derive(Clone, Debug)]
pub struct ThemeExtensions {
    // =========================================================================
    // FILE TYPE COLORS
    // =========================================================================
    /// Folder entries.
    pub file_folder: Color32,
    /// Archive files (.zip, .rar, .7z, etc.)
    pub file_archive: Color32,
    /// Image files (.jpg, .png, .gif, etc.)
    pub file_image: Color32,
    /// Video files (.mp4, .mkv, .avi, etc.)
    pub file_video: Color32,
    /// Audio files (.mp3, .flac, .wav, etc.)
    pub file_audio: Color32,
    /// Plain text and structured-text documents (.txt, .md, .doc, .docx, .toml, etc.)
    pub file_document: Color32,
    /// PDF documents — kept distinct from `file_document` since the
    /// red PDF brand is a near-universal convention.
    pub file_pdf: Color32,
    /// Code / script files (.rs, .py, .js, etc.)
    pub file_code: Color32,
    /// Executable files (.exe, .msi, .bat, etc.)
    pub file_executable: Color32,
    /// Shortcut / link / library files (.url, .lnk, .dll, .so).
    pub file_link: Color32,
    /// Unknown / other files — falls back to surface variant.
    pub file_other: Color32,

    // =========================================================================
    // STATUS/BADGE COLORS
    // =========================================================================
    /// New/unread items
    pub badge_new: Color32,
    /// Updated/modified items
    pub badge_updated: Color32,
    /// Locked/protected items
    pub badge_locked: Color32,
    /// Encrypted items
    pub badge_encrypted: Color32,
}

impl ThemeExtensions {
    /// Create default extensions derived from core colors
    pub fn from_colors(colors: &ThemeColors) -> Self {
        Self {
            // File-type defaults — Tailwind 400-ish palette for legibility on
            // both light and dark surfaces.
            file_folder: Color32::from_rgb(251, 191, 36),     // amber-400
            file_archive: Color32::from_rgb(251, 146, 60),    // orange-400
            file_image: Color32::from_rgb(74, 222, 128),      // green-400
            file_video: Color32::from_rgb(248, 113, 113),     // red-400
            file_audio: Color32::from_rgb(192, 132, 252),     // purple-400
            file_document: Color32::from_rgb(96, 165, 250),   // blue-400
            file_pdf: Color32::from_rgb(239, 68, 68),         // red-500
            file_code: Color32::from_rgb(45, 212, 191),       // teal-400
            file_executable: Color32::from_rgb(96, 165, 250), // blue-400
            file_link: Color32::from_rgb(156, 163, 175),      // gray-400
            file_other: colors.on_surface_variant,

            // Badge colors
            badge_new: colors.tertiary,
            badge_updated: colors.info,
            badge_locked: colors.warning,
            badge_encrypted: colors.error,
        }
    }

    /// Create with custom overrides
    pub fn with_file_archive(mut self, color: Color32) -> Self {
        self.file_archive = color;
        self
    }

    pub fn with_file_image(mut self, color: Color32) -> Self {
        self.file_image = color;
        self
    }

    pub fn with_file_video(mut self, color: Color32) -> Self {
        self.file_video = color;
        self
    }

    pub fn with_file_audio(mut self, color: Color32) -> Self {
        self.file_audio = color;
        self
    }

    pub fn with_file_document(mut self, color: Color32) -> Self {
        self.file_document = color;
        self
    }

    pub fn with_file_code(mut self, color: Color32) -> Self {
        self.file_code = color;
        self
    }
}
