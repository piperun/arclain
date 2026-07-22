//! Context and types for plugin UI rendering

use crate::shared::image_assets::ImageOwner;
use crate::shared::{theme::ThemeColors, SharedState};

/// Callback trait
pub trait UiEventHandler: FnMut(&str, Option<String>) {}
impl<T: FnMut(&str, Option<String>)> UiEventHandler for T {}

/// Boxed callback for storage/passing
pub type UiEventCallback<'a> = Box<dyn UiEventHandler + 'a>;

/// Context passed down during recursive rendering
pub struct RenderContext<'a, H: UiEventHandler + ?Sized> {
    pub event_callback: &'a mut H,
    pub colors: &'a ThemeColors,
    pub shared_state: Option<&'a SharedState>,
    pub plugin_id: Option<&'a str>,
    pub image_owner: Option<&'a ImageOwner>,
}
