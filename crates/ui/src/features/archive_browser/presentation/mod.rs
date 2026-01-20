//! Presentation layer for archive browser

pub mod components;
pub mod controllers;
pub mod views;

pub use controllers::browser_controller::BrowserController;
pub use views::browser_page::render_archive_browser;
