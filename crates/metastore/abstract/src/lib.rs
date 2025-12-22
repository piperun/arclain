//! Abstraction traits for metastore
//!
//! These traits define the interfaces that backends must implement.

mod database;
mod http;
mod ui;

pub use database::*;
pub use http::*;
pub use ui::*;
