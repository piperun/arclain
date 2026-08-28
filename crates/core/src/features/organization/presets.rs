//! The layouts arclain ships.
//!
//! Named by shape rather than by game: the distinguishing property is
//! the format the result has to be in, and a layout tuned for one title
//! is user-side data, not something this repository carries.

use super::layout::{
    FetchSource, Fetched, FileVariable, Generated, GeneratedContent, Layout, OutputSelector,
    Placement, Source,
};

/// One storefront product: the payload under `Game/`, the metadata
/// document beside it, the store screenshots fetched in. `name` is the
/// folder template, which differs between the rules that seed a fresh
/// database and the preset offered in the rules page, so it is the one
/// thing this takes.
///
/// `image_$index.$ext` is exactly the naming this shape has always had:
/// `$index` counts from one padded to three digits, and `$ext` follows
/// the source URL rather than the template, so a `.png` is not saved
/// under a `.jpg` name.
pub fn product_layout(name: &str) -> Layout {
    Layout {
        outputs: OutputSelector::Whole,
        file_variables: vec![],
        name: name.to_string(),
        place: vec![Placement {
            from: Source::ContentRoot,
            into: "Game".to_string(),
        }],
        generate: vec![Generated {
            into: "metadata.json".to_string(),
            content: GeneratedContent::MetadataDocument,
        }],
        fetch: vec![Fetched {
            into: "screenshots".to_string(),
            source: FetchSource::Screenshots,
            name: "image_$index.$ext".to_string(),
        }],
    }
}

/// A pack of mods, which is not one thing: every folder holding a
/// `modinfo.ini` is its own output, named by what that file calls it,
/// with no wrapper above the set. A mod manager then reads the result as
/// the several mods it is rather than as one mod full of rubbish.
pub fn mod_manager_layout() -> Layout {
    Layout {
        outputs: OutputSelector::PerDirectoryContaining {
            marker: "modinfo.ini".to_string(),
        },
        file_variables: vec![FileVariable {
            as_name: "mod_name".to_string(),
            file: "modinfo.ini".to_string(),
            key: "name".to_string(),
        }],
        name: "$mod_name".to_string(),
        place: vec![Placement {
            from: Source::All,
            into: String::new(),
        }],
        generate: vec![],
        fetch: vec![],
    }
}
