// Organization feature module

pub mod add_rule_dialog;
pub mod operations;
pub mod organize_panel;
pub mod rules_page;
pub mod state;
pub mod ui;

// Re-export commonly used types
pub use organize_panel::{OrganizePanel, OrganizePanelAction};
pub use ui::{OrganizationAction, OrganizationFeature};
