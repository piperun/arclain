//! Where a plugin's vocabulary becomes a number.
//!
//! A plugin names a role, a step or a size hint; it never names pixels. This
//! module holds every table that answers those names, plus the two layout
//! numbers nobody asks for at all (the carousel's thumbnail strip and the
//! cap on a `Split` in a stacked host). One name is answered elsewhere: a
//! badge level resolves to a colour off the live theme, not to a constant,
//! so it lives with the chrome that draws it in
//! `shared::components::top_tab_bar`. Changing a value here restyles every
//! plugin at once, which is the whole reason the vocabulary is coarse.
//!
//! It sits beside [`super::document`] rather than inside it because none of
//! this is rendering: no `Ui`, no widget, no event. The renderer reads these
//! tables; the tests read them too, which is what makes them worth naming.

use arclain_app::plugins::{SidebarWidth, SizeHint, SpacingStep, TextRole};

/// One row of arclain's type scale: the numbers a [`TextRole`] resolves
/// to. Not to be confused with [`eframe::egui::TextStyle`], which is a role
/// key rather than the resolved values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoleStyle {
    pub size: f32,
    pub bold: bool,
    /// Opens a section, and is given room above and below to do it.
    pub heading: bool,
}

/// arclain's own type scale. A plugin names a role; this is where the role
/// becomes a number, and changing these values restyles every plugin at once.
pub fn text_style_for_role(role: TextRole) -> RoleStyle {
    let (size, bold, heading) = match role {
        TextRole::Title => (18.0, true, true),
        TextRole::Subtitle => (16.0, true, true),
        TextRole::Emphasis => (14.0, true, false),
        TextRole::Body => (14.0, false, false),
        TextRole::Caption => (12.0, false, false),
    };
    RoleStyle {
        size,
        bold,
        heading,
    }
}

/// arclain's own vertical scale, for an image. A plugin names a step; this
/// is where the step becomes a number. `None` keeps its meaning of "the
/// host decides", which for an image is [`super::image::render_texture`]'s
/// own cap.
///
/// The three `*_for_hint` functions are deliberately separate rather than
/// one function taking a kind: the same step is a different number for each
/// of them, and only the image has an answer for the absent case that the
/// renderer below it can act on.
pub fn image_height_for_hint(hint: Option<SizeHint>) -> Option<f32> {
    hint.map(|hint| match hint {
        SizeHint::Compact => 150.0,
        SizeHint::Regular => 200.0,
        SizeHint::Tall => 400.0,
    })
}

/// arclain's own vertical scale, for a scrolling list container. A list
/// always has a height, so an absent hint resolves to the middle of the
/// scale rather than to nothing.
pub fn list_height_for_hint(hint: Option<SizeHint>) -> f32 {
    match hint {
        Some(SizeHint::Compact) => 200.0,
        Some(SizeHint::Regular) | None => 300.0,
        Some(SizeHint::Tall) => 700.0,
    }
}

/// arclain's own vertical scale, for a carousel's main image area. A
/// carousel always has a height. Note that `Regular` is 400 here and 200
/// for an image -- that difference is the whole reason a plugin names the
/// step instead of the number.
pub fn carousel_height_for_hint(hint: Option<SizeHint>) -> f32 {
    match hint {
        Some(SizeHint::Compact) => 200.0,
        Some(SizeHint::Regular) => 400.0,
        Some(SizeHint::Tall) => 700.0,
        None => 300.0,
    }
}

/// arclain's own horizontal scale, for a split's sidebar. A sidebar always
/// has a width, so an absent step resolves to the middle of the scale
/// rather than to nothing.
pub fn sidebar_width_for_step(step: Option<SidebarWidth>) -> f32 {
    match step {
        Some(SidebarWidth::Narrow) => 200.0,
        Some(SidebarWidth::Medium) | None => 250.0,
        Some(SidebarWidth::Wide) => 300.0,
    }
}

/// arclain's own spacing scale, for the gap a plugin asks for between two
/// elements. A step is never absent here -- the vocabulary makes a plugin
/// name one -- so this takes the step itself rather than an option.
pub fn space_height_for_step(step: SpacingStep) -> f32 {
    match step {
        SpacingStep::Small => 8.0,
        SpacingStep::Medium => 12.0,
        SpacingStep::Large => 20.0,
    }
}

/// The carousel's thumbnail strip is the host's decision entirely, not
/// something a plugin can ask about. Stated here rather than left to
/// `CarouselConfig`'s own default so that changing how a bare carousel
/// looks elsewhere in the app cannot silently move a plugin's.
pub(super) const CAROUSEL_THUMBNAIL_HEIGHT: f32 = 60.0;

/// Default cap for a document hosted in the archive browser's properties
/// panel. Sized to show a usable two-pane layout without dominating a
/// panel that also stacks archive info, file info, and attributes.
pub const PANEL_SPLIT_MAX_HEIGHT: u32 = 320;
