//! Theme extensions for optional helper colors
//!
//! These are secondary colors that can be customized by users/plugins
//! but have sensible defaults derived from the core theme.

use egui::Color32;

use crate::ThemeColors;

/// Extension colors for specific use cases (file types, status badges, etc.)
///
/// These derive from core theme colors by default but can be overridden.
#[derive(Clone, Debug)]
pub struct ThemeExtensions {
    // =========================================================================
    // FILE TYPE COLORS
    // =========================================================================
    /// Archive files (.zip, .rar, .7z, etc.)
    pub file_archive: Color32,
    /// Image files (.jpg, .png, .gif, etc.)
    pub file_image: Color32,
    /// Video files (.mp4, .mkv, .avi, etc.)
    pub file_video: Color32,
    /// Audio files (.mp3, .flac, .wav, etc.)
    pub file_audio: Color32,
    /// Document files (.pdf, .doc, .txt, etc.)
    pub file_document: Color32,
    /// Code/script files (.rs, .py, .js, etc.)
    pub file_code: Color32,
    /// Executable files (.exe, .app, etc.)
    pub file_executable: Color32,
    /// Unknown/other files
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
            // File type colors (semantic defaults)
            file_archive: Color32::from_rgb(255, 193, 7), // amber
            file_image: Color32::from_rgb(233, 30, 99),   // pink
            file_video: Color32::from_rgb(156, 39, 176),  // purple
            file_audio: Color32::from_rgb(0, 188, 212),   // cyan
            file_document: Color32::from_rgb(33, 150, 243), // blue
            file_code: Color32::from_rgb(76, 175, 80),    // green
            file_executable: Color32::from_rgb(244, 67, 54), // red
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
