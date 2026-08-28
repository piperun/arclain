use serde::{Deserialize, Serialize};

/// What counts as one output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OutputSelector {
    /// The whole input is one output.
    Whole,
    /// One output per directory that directly contains `marker`.
    PerDirectoryContaining { marker: String },
}

/// Variables read out of files inside the input, resolved once per
/// output, usable in `name` and in any `into`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileVariable {
    /// Name the template refers to, without the `$`.
    pub as_name: String,
    /// Path of the file to read, relative to the output's own root.
    pub file: String,
    /// Key to take from it.
    pub key: String,
}

/// Where each output's content goes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub from: Source,
    /// Destination inside the output. Empty means the output's root.
    pub into: String,
}

/// The source of files to place in an output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Source {
    /// Everything under this output's root.
    All,
    /// Paths matching a glob, relative to the output's root.
    Matching(String),
    /// The folder that looks like the payload, by indicator scoring.
    ContentRoot,
}

/// A file written into each output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Generated {
    pub into: String,
    pub content: GeneratedContent,
}

/// The kind of file content to generate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GeneratedContent {
    /// The layered document the metadata provider produced.
    MetadataDocument,
}

/// An image to fetch into each output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fetched {
    pub into: String,
    pub source: FetchSource,
    /// Template for each file's name. Two tokens beyond the output's own
    /// variables: `$index` is the item's position in the source list,
    /// counted from one and padded to three digits so ten of them still
    /// sort in order, and `$ext` is the extension the item's source URL
    /// carries, or `jpg` when it names none. A name has to be able to
    /// say `$ext`, because a template that spells an extension out
    /// renames a `.png` to `.jpg` and no amount of care in the template
    /// can tell what the URL will be.
    pub name: String,
}

/// The source of images to fetch into an output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FetchSource {
    Screenshots,
}

/// A fixed set of typed fields describing how to organize an archive's output.
/// Not an ordered directive list, and not an expression language.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    /// What counts as one output.
    pub outputs: OutputSelector,
    /// Variables read out of files inside the input, resolved once per
    /// output, usable in `name` and in any `into`.
    pub file_variables: Vec<FileVariable>,
    /// Template for each output's root folder name. An empty template
    /// means the output has no wrapper and its content sits at the top
    /// level of the result.
    pub name: String,
    /// Where each output's content goes.
    pub place: Vec<Placement>,
    /// Files written into each output.
    pub generate: Vec<Generated>,
    /// Images fetched into each output.
    pub fetch: Vec<Fetched>,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            outputs: OutputSelector::Whole,
            file_variables: vec![],
            name: String::new(),
            place: vec![],
            generate: vec![],
            fetch: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two shapes the vocabulary exists to express. If either cannot
    /// be written down, the vocabulary is wrong.
    #[test]
    fn both_shipped_shapes_round_trip() {
        let product = Layout {
            outputs: OutputSelector::Whole,
            file_variables: vec![],
            name: "[$product_id][$circle] $title".to_string(),
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
        };

        let mod_manager = Layout {
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
        };

        for layout in [product, mod_manager] {
            let json = serde_json::to_string(&layout).expect("serialize");
            let back: Layout = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, layout);
        }
    }

    #[test]
    fn a_default_layout_is_one_unnamed_output_placing_nothing() {
        let layout = Layout::default();
        assert_eq!(layout.outputs, OutputSelector::Whole);
        assert!(layout.name.is_empty());
        assert!(layout.place.is_empty());
    }
}
