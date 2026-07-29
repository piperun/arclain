//! egui integration for `arclain_app::Signal` (a re-export of
//! `arclain_signals::Signal`) -- binds a signal to egui's repaint system.
//!
//! Used to live in `arclain_signals` itself, behind an `egui` feature:
//! moved here because the frontend/headless boundary guard's source scan
//! flags any reference to `egui`/`eframe` inside a headless crate's
//! source tree regardless of `#[cfg(feature = ...)]` gating, so the mere
//! existence of egui-integration code in that crate was itself a
//! violation, feature-gated or not. `arclain_ui` already depends on egui
//! directly (it is the GUI shell), so this is exactly where such
//! toolkit-specific glue belongs -- `arclain_signals` stays genuinely
//! headless, with no optional GUI dependency at all.

use arclain_app::Signal;

/// Context for binding signals to egui's repaint system.
///
/// # Example
/// ```ignore
/// use arclain_app::Signal;
/// use crate::core::signal_context::SignalContext;
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

    /// Bind a signal with a name (for debugging). Logs which signal triggers repaint.
    pub fn bind_named<T: Clone + Send + Sync + 'static>(
        &self,
        signal: &Signal<T>,
        name: &'static str,
    ) {
        let ctx = self.ctx.clone();
        signal.subscribe(move || {
            tracing::trace!("[SIGNAL] {} triggered repaint", name);
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
