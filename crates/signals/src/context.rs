//! egui integration - bind signals to UI repaint
//!
//! This module is only available with the `egui` feature.

use crate::Signal;

/// Context for binding signals to egui's repaint system.
///
/// # Example
/// ```ignore
/// use arclain_signals::{Signal, SignalContext};
///
/// fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
///     let signal_ctx = SignalContext::new(ctx.clone());
///     signal_ctx.bind(&self.my_signal);
///     
///     // Now when my_signal.set() is called, egui will repaint
///     egui::CentralPanel::default().show(ctx, |ui| {
///         ui.label(format!("Value: {}", self.my_signal.get()));
///     });
/// }
/// ```
pub struct SignalContext {
    ctx: egui::Context,
}

impl SignalContext {
    /// Create a new signal context from an egui context.
    pub fn new(ctx: egui::Context) -> Self {
        Self { ctx }
    }

    /// Bind a signal to this context. When the signal changes, egui will repaint.
    pub fn bind<T: Clone + Send + Sync + 'static>(&self, signal: &Signal<T>) {
        let ctx = self.ctx.clone();
        signal.subscribe(move || {
            ctx.request_repaint();
        });
    }

    /// Get the underlying egui context.
    pub fn egui_context(&self) -> &egui::Context {
        &self.ctx
    }
}

impl Clone for SignalContext {
    fn clone(&self) -> Self {
        Self {
            ctx: self.ctx.clone(),
        }
    }
}
