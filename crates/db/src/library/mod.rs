pub mod content;
#[cfg(feature = "gameta")]
pub mod metadata;
pub mod migration;

#[cfg(feature = "gameta")]
pub use metadata::{CompletenessScore, MetadataSource, ProductMetadata};

pub use content::{
    delete_product_content, get_all_content, get_cover, get_screenshots,
    save as save_product_content, ContentType, ProductContent,
};

#[cfg(test)]
mod tests;
