//! GStreamer Media Preview Plugin
//!
//! This plugin provides media file previews and thumbnail generation
//! using a hybrid WASM coordinator + native GStreamer service approach.
//!
//! The WASM plugin acts as a coordinator while delegating heavy media
//! processing to a native GStreamer service via IPC.

#![no_std]

extern crate alloc;

#[macro_use]
extern crate archust_plugin_sdk;

use alloc::format;
use alloc::string::ToString;
use archust_plugin_sdk::prelude::*;
use serde_json::json;

plugin_metadata!(
    "gstreamer-preview",
    "GStreamer Media Preview",
    "0.1.0",
    "Archust Team",
    "Provides media file previews and thumbnail generation"
);

plugin_init!();
plugin_cleanup!();

// UI Layout for extension points
plugin_ui_layout!(|extension_point| {
    use alloc::vec;
    use alloc::vec::Vec;

    match extension_point {
        PluginExtensionPoint::MainPage => {
            vec![
                PluginUiElement::Label {
                    text: "GStreamer Media Preview Settings".to_string(),
                    bold: true,
                    size: Some(18.0),
                },
                PluginUiElement::Space { size: 12.0 },
                PluginUiElement::Separator,
                PluginUiElement::Space { size: 12.0 },
                PluginUiElement::Label {
                    text: "Supported Formats".to_string(),
                    bold: true,
                    size: Some(14.0),
                },
                PluginUiElement::Space { size: 8.0 },
                PluginUiElement::Checkbox {
                    id: "enable_video".to_string(),
                    label: "Video files (MP4, MKV, AVI, etc.)".to_string(),
                    checked: true,
                },
                PluginUiElement::Space { size: 4.0 },
                PluginUiElement::Checkbox {
                    id: "enable_audio".to_string(),
                    label: "Audio files (MP3, FLAC, WAV, etc.)".to_string(),
                    checked: true,
                },
                PluginUiElement::Space { size: 16.0 },
                PluginUiElement::Label {
                    text: "Thumbnail Generation".to_string(),
                    bold: true,
                    size: Some(14.0),
                },
                PluginUiElement::Space { size: 8.0 },
                PluginUiElement::Checkbox {
                    id: "auto_generate".to_string(),
                    label: "Automatically generate thumbnails on archive open".to_string(),
                    checked: false,
                },
                PluginUiElement::Space { size: 8.0 },
                PluginUiElement::Label {
                    text: "Note: Automatic generation may slow down archive opening.".to_string(),
                    bold: false,
                    size: Some(12.0),
                },
            ]
        }
        PluginExtensionPoint::Sidebar => {
            vec![
                PluginUiElement::Label {
                    text: "Media Preview".to_string(),
                    bold: true,
                    size: Some(14.0),
                },
                PluginUiElement::Space { size: 8.0 },
                PluginUiElement::Separator,
                PluginUiElement::Space { size: 8.0 },
                PluginUiElement::Row {
                    children: vec![
                        PluginUiElement::Label {
                            text: "Media files:".to_string(),
                            bold: true,
                            size: None,
                        },
                        PluginUiElement::Space { size: 4.0 },
                        PluginUiElement::Label {
                            text: "0 found".to_string(),
                            bold: false,
                            size: None,
                        },
                    ],
                },
                PluginUiElement::Space { size: 8.0 },
                PluginUiElement::Button {
                    id: "generate_thumbnails".to_string(),
                    label: "Generate Thumbnails".to_string(),
                },
            ]
        }
        _ => Vec::new(),
    }
});

// UI Event handler
plugin_ui_event!(|id, value| {
    log(
        LogLevel::Info,
        &format!("GStreamer UI event: {} = {:?}", id, value),
    );

    match id {
        "generate_thumbnails" => {
            log(LogLevel::Info, "Generating thumbnails...");
            // TODO: Implement thumbnail generation
        }
        "enable_video" => {
            if let Some(val) = value {
                log(
                    LogLevel::Info,
                    &format!("Video support changed to: {}", val),
                );
                // TODO: Update video support setting
            }
        }
        "enable_audio" => {
            if let Some(val) = value {
                log(
                    LogLevel::Info,
                    &format!("Audio support changed to: {}", val),
                );
                // TODO: Update audio support setting
            }
        }
        "auto_generate" => {
            if let Some(val) = value {
                log(
                    LogLevel::Info,
                    &format!("Auto-generate thumbnails changed to: {}", val),
                );
                // TODO: Update auto-generate setting
            }
        }
        _ => {
            log(LogLevel::Debug, &format!("Unknown UI event: {}", id));
        }
    }
});

/// Supported media file extensions
const SUPPORTED_VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg",
];

const SUPPORTED_AUDIO_EXTENSIONS: &[&str] =
    &["mp3", "wav", "flac", "ogg", "m4a", "aac", "wma", "opus"];

/// Check if file is a supported media file
pub fn is_media_file(filename: &str) -> bool {
    if let Some(ext_pos) = filename.rfind('.') {
        let ext = &filename[ext_pos + 1..].to_lowercase();
        SUPPORTED_VIDEO_EXTENSIONS.contains(&ext.as_str())
            || SUPPORTED_AUDIO_EXTENSIONS.contains(&ext.as_str())
    } else {
        false
    }
}

/// Check if file is a video
pub fn is_video_file(filename: &str) -> bool {
    if let Some(ext_pos) = filename.rfind('.') {
        let ext = &filename[ext_pos + 1..].to_lowercase();
        SUPPORTED_VIDEO_EXTENSIONS.contains(&ext.as_str())
    } else {
        false
    }
}

/// Check if file is an audio file
pub fn is_audio_file(filename: &str) -> bool {
    if let Some(ext_pos) = filename.rfind('.') {
        let ext = &filename[ext_pos + 1..].to_lowercase();
        SUPPORTED_AUDIO_EXTENSIONS.contains(&ext.as_str())
    } else {
        false
    }
}

/// Plugin event handler
///
/// This plugin responds to archive open events by identifying media files
/// and preparing metadata about them. Actual media processing (thumbnails,
/// duration extraction) would be delegated to a native GStreamer service.
#[no_mangle]
pub extern "C" fn plugin_on_event(event_ptr: *const u8, event_len: usize) -> i32 {
    // Read event JSON from memory
    let event_bytes = unsafe { core::slice::from_raw_parts(event_ptr, event_len) };

    let event_str = match core::str::from_utf8(event_bytes) {
        Ok(s) => s,
        Err(_) => {
            log(LogLevel::Error, "Invalid UTF-8 in event");
            return -1;
        }
    };

    // Parse event
    let event: PluginEvent = match serde_json::from_str(event_str) {
        Ok(e) => e,
        Err(_) => {
            log(LogLevel::Error, "Failed to parse event JSON");
            return -1;
        }
    };

    // Handle OnArchiveOpen event
    match event {
        PluginEvent::OnArchiveOpen { ref path, .. } => {
            log(LogLevel::Info, &format!("Archive opened: {}", path));

            // Note: In a full implementation, we would:
            // 1. List all files in the archive using file_list() host function
            // 2. Identify media files
            // 3. Send thumbnail generation requests to native GStreamer service
            // 4. Cache results
            // 5. Return metadata with thumbnail paths

            // For now, just log that we received the event
            log(
                LogLevel::Info,
                "GStreamer preview plugin loaded (stub implementation)",
            );

            0
        }
        PluginEvent::OnFileExtract { ref file_path, .. } => {
            // Check if extracted file is a media file
            if is_media_file(file_path) {
                log(
                    LogLevel::Info,
                    &format!("Media file extracted: {}", file_path),
                );

                if is_video_file(file_path) {
                    log(
                        LogLevel::Debug,
                        "File is a video - thumbnail generation would be triggered",
                    );
                } else if is_audio_file(file_path) {
                    log(
                        LogLevel::Debug,
                        "File is audio - metadata extraction would be triggered",
                    );
                }
            }

            0
        }
        _ => 0, // Ignore other events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_video_file() {
        assert!(is_video_file("movie.mp4"));
        assert!(is_video_file("VIDEO.MKV"));
        assert!(is_video_file("clip.avi"));
        assert!(!is_video_file("audio.mp3"));
        assert!(!is_video_file("document.pdf"));
    }

    #[test]
    fn test_is_audio_file() {
        assert!(is_audio_file("song.mp3"));
        assert!(is_audio_file("MUSIC.FLAC"));
        assert!(is_audio_file("audio.wav"));
        assert!(!is_audio_file("video.mp4"));
        assert!(!is_audio_file("document.pdf"));
    }

    #[test]
    fn test_is_media_file() {
        assert!(is_media_file("video.mp4"));
        assert!(is_media_file("audio.mp3"));
        assert!(is_media_file("MOVIE.MKV"));
        assert!(!is_media_file("document.pdf"));
        assert!(!is_media_file("image.jpg"));
    }
}
