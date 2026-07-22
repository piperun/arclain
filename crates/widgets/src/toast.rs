//! Toast notification widget for displaying temporary messages
//!
//! Provides a toast notification system with:
//! - Multiple severity levels (Info, Success, Warning, Error)
//! - Auto-dismiss after configurable duration
//! - Stacked display in corner of screen

use eframe::egui;
use std::time::{Duration, Instant};

/// How long a freshly-created toast stays fully visible before
/// starting to fade out.
const DEFAULT_TOAST_DURATION: Duration = Duration::from_millis(3000);

/// Length of the fade-out animation at the end of a toast's lifetime.
const TOAST_FADE_DURATION: Duration = Duration::from_millis(300);

/// Toast severity level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastLevel {
    Info,
    Success,
    Warning,
    Error,
}

impl ToastLevel {
    /// Get the icon for this level
    pub fn icon(&self) -> &'static str {
        match self {
            ToastLevel::Info => "ℹ",
            ToastLevel::Success => "✓",
            ToastLevel::Warning => "⚠",
            ToastLevel::Error => "✕",
        }
    }

    /// Get the default color for this level
    pub fn color(&self) -> egui::Color32 {
        match self {
            ToastLevel::Info => egui::Color32::from_rgb(66, 165, 245), // Blue
            ToastLevel::Success => egui::Color32::from_rgb(102, 187, 106), // Green
            ToastLevel::Warning => egui::Color32::from_rgb(255, 167, 38), // Orange
            ToastLevel::Error => egui::Color32::from_rgb(239, 83, 80), // Red
        }
    }
}

/// A single toast notification
#[derive(Debug, Clone)]
pub struct Toast {
    pub level: ToastLevel,
    pub message: String,
    pub duration: Duration,
    pub shown_at: Option<Instant>,
}

impl Toast {
    /// Create a new toast
    pub fn new(level: ToastLevel, message: impl Into<String>) -> Self {
        Self {
            level,
            message: message.into(),
            duration: DEFAULT_TOAST_DURATION,
            shown_at: None,
        }
    }

    /// Set custom duration
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }

    /// Create an info toast
    pub fn info(message: impl Into<String>) -> Self {
        Self::new(ToastLevel::Info, message)
    }

    /// Create a success toast
    pub fn success(message: impl Into<String>) -> Self {
        Self::new(ToastLevel::Success, message)
    }

    /// Create a warning toast
    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(ToastLevel::Warning, message)
    }

    /// Create an error toast
    pub fn error(message: impl Into<String>) -> Self {
        Self::new(ToastLevel::Error, message)
    }

    /// Check if this toast has expired
    pub fn is_expired(&self) -> bool {
        self.shown_at
            .map(|t| t.elapsed() >= self.duration)
            .unwrap_or(false)
    }

    /// Mark as shown (start timer)
    pub fn mark_shown(&mut self) {
        if self.shown_at.is_none() {
            self.shown_at = Some(Instant::now());
        }
    }
}

/// Container for managing multiple toasts
#[derive(Debug, Default)]
pub struct Toaster {
    toasts: Vec<Toast>,
}

impl Toaster {
    /// Create a new toaster
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a toast to the queue
    pub fn add(&mut self, toast: Toast) {
        self.toasts.push(toast);
    }

    /// Add an info toast
    pub fn info(&mut self, message: impl Into<String>) {
        self.add(Toast::info(message));
    }

    /// Add a success toast
    pub fn success(&mut self, message: impl Into<String>) {
        self.add(Toast::success(message));
    }

    /// Add a warning toast
    pub fn warning(&mut self, message: impl Into<String>) {
        self.add(Toast::warning(message));
    }

    /// Add an error toast
    pub fn error(&mut self, message: impl Into<String>) {
        self.add(Toast::error(message));
    }

    /// Render all active toasts (call this in your update loop)
    ///
    /// Displays toasts in the bottom-right corner, stacked vertically.
    pub fn show(&mut self, ctx: &egui::Context) {
        // Remove expired toasts
        self.toasts.retain(|t| !t.is_expired());

        if self.toasts.is_empty() {
            return;
        }

        // Request repaint for animation
        ctx.request_repaint();

        let margin = 16.0;
        let toast_width = 320.0;
        let spacing = 8.0;
        let toast_count = self.toasts.len();

        egui::Area::new(egui::Id::new("toaster_area"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-margin, -margin))
            .show(ctx, |ui| {
                ui.set_width(toast_width);

                for (i, toast) in self.toasts.iter_mut().enumerate() {
                    toast.mark_shown();

                    // Calculate opacity for fade out
                    let remaining = toast
                        .shown_at
                        .map(|t| toast.duration.saturating_sub(t.elapsed()))
                        .unwrap_or(toast.duration);
                    let opacity = if remaining < TOAST_FADE_DURATION {
                        remaining.as_secs_f32() / TOAST_FADE_DURATION.as_secs_f32()
                    } else {
                        1.0
                    };

                    let bg_color =
                        egui::Color32::from_rgba_unmultiplied(40, 44, 52, (220.0 * opacity) as u8);
                    let accent = toast.level.color();

                    let frame_response = egui::Frame::NONE
                        .fill(bg_color)
                        .stroke(egui::Stroke::new(2.0_f32, accent.gamma_multiply(opacity)))
                        .corner_radius(8.0)
                        .inner_margin(egui::Margin::symmetric(12, 10))
                        .show(ui, |ui| {
                            ui.set_min_width(toast_width - 24.0);
                            ui.horizontal(|ui| {
                                // Icon
                                ui.label(
                                    egui::RichText::new(toast.level.icon())
                                        .size(16.0)
                                        .color(accent.gamma_multiply(opacity)),
                                );
                                ui.add_space(8.0);
                                // Message
                                ui.label(egui::RichText::new(&toast.message).size(13.0).color(
                                    egui::Color32::from_white_alpha((240.0 * opacity) as u8),
                                ));
                            });
                        });

                    // Toast is data, not a builder — no per-instance
                    // `debug_lines` builder, so the overlay is purely
                    // env-flag driven. Useful when toasts pile up off
                    // the bottom edge or when the fade math drifts.
                    #[cfg(debug_assertions)]
                    crate::debug::paint_widget_rect_debug(
                        ui.painter(),
                        frame_response.response.rect,
                        &format!("toast[{}]", i),
                        crate::debug::ui_debug_guidelines_enabled(),
                    );

                    if i < toast_count - 1 {
                        ui.add_space(spacing);
                    }
                }
            });
    }
}
