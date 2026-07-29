//! Flutter-like reactive signals for Rust
//!
//! This crate provides a simple, GUI-agnostic reactive state system inspired by
//! Flutter Signals and SolidJS signals. Deliberately headless: no GUI
//! toolkit dependency of any kind, not even an optional one, so any
//! frontend can build its own repaint-binding glue on top of [`Signal<T>`]
//! -- see `arclain_ui::core::signal_context::SignalContext` for the egui
//! one, which used to live here behind an `egui` feature until that
//! feature's mere existence (even unused) tripped this workspace's
//! frontend/headless boundary guard's source scan (it flags any
//! reference to `egui`/`eframe` inside a headless crate's source tree,
//! regardless of `#[cfg(feature = ...)]` gating).
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

mod computed;
mod effect;
mod signal;

pub use computed::Computed;
pub use effect::Effect;
pub use signal::Signal;
