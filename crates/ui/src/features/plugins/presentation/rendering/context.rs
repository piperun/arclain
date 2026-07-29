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

impl<H: UiEventHandler + ?Sized> RenderContext<'_, H> {
    /// The subset the shared image helpers need -- see
    /// [`super::image::ImageContext`] for why they take that rather than
    /// a whole `RenderContext`.
    pub(super) fn image_context(&self) -> super::image::ImageContext<'_> {
        super::image::ImageContext {
            shared_state: self.shared_state,
            plugin_id: self.plugin_id,
            image_owner: self.image_owner,
        }
    }
}
