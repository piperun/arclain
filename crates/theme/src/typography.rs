//! Semantic font-size scale (TypeScale)
//!
//! Replaces the audit's "254 hardcoded `size(N.0)` font sizes across
//! 59 files" with a small set of named values. Use as
//! `RichText::new(label).size(typography::BODY)`.
//!
//! Sizes are tuned to the existing usage in the codebase (10–24 px
//! range) rather than imported wholesale from a Material Design
//! scale — most arclain UI is dense desktop content, not phone-style
//! display text.

/// Tiny labels: capability tags, badge microcopy, table-header
/// captions inside dense rows.
pub const MICRO: f32 = 10.0;

/// Caption / secondary info: subtitles inside cards, file metadata
/// labels.
pub const CAPTION: f32 = 11.0;

/// UI control labels and dense list items: list-row text,
/// dropdown/checkbox labels.
pub const LABEL: f32 = 12.0;

/// Standard chip/control inline text.
pub const CHIP: f32 = 13.0;

/// Body / form-row text — the dominant size in settings pages and
/// dialog content.
pub const BODY: f32 = 14.0;

/// Subtitle — slightly emphasized labels above an input or section.
pub const SUBTITLE: f32 = 16.0;

/// Section / dialog title.
pub const TITLE: f32 = 18.0;

/// Page heading / panel header.
pub const HEADING: f32 = 24.0;
