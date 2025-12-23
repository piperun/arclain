//! Flutter-like reactive signals for Rust
//!
//! This crate provides a simple, GUI-agnostic reactive state system inspired by
//! Flutter Signals and SolidJS signals.
//!
//! # Core Types
//!
//! - [`Signal<T>`] - Mutable reactive value with change listeners
//! - [`Computed<T>`] - Derived value that auto-updates when dependencies change
//! - [`Effect`] - Side effect that runs when dependencies change
//!
//! # Example
//!
//! ```rust
//! use arclain_signals::Signal;
//!
//! let count = Signal::new(0);
//! count.subscribe(|| println!("Count changed!"));
//! count.set(1); // Prints: Count changed!
//! ```
//!
//! # egui Integration
//!
//! With the `egui` feature, you can bind signals to egui's repaint system:
//!
//! ```rust,ignore
//! use arclain_signals::{Signal, SignalContext};
//!
//! let signal = Signal::new(42);
//! let ctx = SignalContext::new(egui_ctx);
//! ctx.bind(&signal); // Signal changes will trigger repaint
//! ```

mod computed;
mod effect;
mod signal;

#[cfg(feature = "egui")]
mod context;

pub use computed::Computed;
pub use effect::Effect;
pub use signal::Signal;

#[cfg(feature = "egui")]
pub use context::SignalContext;
