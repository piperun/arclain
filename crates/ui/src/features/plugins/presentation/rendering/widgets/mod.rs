//! Plugin widget renderers.
//!
//! Was a single 740-LOC file. Split here by interaction category so
//! each sub-module stays under ~300 LOC and concerns don't bleed
//! between input handling, container chrome, and display-only
//! widgets:
//!
//! - [`form`] — interactive input controls. User changes a value,
//!   plugin gets an event (button, text input, checkbox, radio group,
//!   slider, dropdown).
//! - [`containers`] — structural wrappers that group other widgets
//!   (tabs, toolbar, settings group, section header).
//! - [`display`] — read-only data presentation widgets (label,
//!   warning, loading spinner, tag chips, list item, key/value list,
//!   metadata grid).

mod containers;
mod display;
mod form;

pub use containers::{render_section_header, render_settings_group, render_tabs, render_toolbar};
pub use display::{
    render_key_value_list, render_label, render_list_item, render_loading, render_metadata_grid,
    render_tag_chips, render_warning,
};
pub use form::{
    render_button, render_checkbox, render_dropdown, render_radio_group, render_slider,
    render_text_input,
};
