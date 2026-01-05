//! Library (product) feature module

mod content;
mod metadata;

pub use metadata::{
    delete as delete_product_metadata, get_by_external_id, init_product_metadata_schema,
    list_by_source, load as load_product_metadata, save as save_product_metadata, MetadataSource,
    ProductMetadata,
};

pub use content::{
    delete_product_content, get_all_content, get_cover, get_screenshots,
    init_product_content_schema, save as save_product_content, ContentType, ProductContent,
};
