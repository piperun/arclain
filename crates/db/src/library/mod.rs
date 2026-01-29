pub mod content;
pub mod metadata;

pub use metadata::{
    delete, get_by_external_id, list_by_source, list_ids_by_source, load,
    migrate_repair_extras_json, save, MetadataSource, ProductMetadata,
};

pub use content::{
    delete_product_content, get_all_content, get_cover, get_screenshots,
    save as save_product_content, ContentType, ProductContent,
};

#[cfg(test)]
mod tests;
