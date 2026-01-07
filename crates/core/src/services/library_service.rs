use anyhow::Result;
use arclain_db::library::metadata::{self, MetadataSource, ProductMetadata};
use arclain_db::DieselPool;

pub struct LibraryService {
    pool: DieselPool,
}

impl LibraryService {
    pub fn new(pool: DieselPool) -> Self {
        Self { pool }
    }

    pub fn save_metadata(&self, meta: &ProductMetadata) -> Result<()> {
        self.pool.with_conn(|conn| metadata::save(conn, meta))
    }

    pub fn get_metadata(&self, id: &str) -> Result<Option<ProductMetadata>> {
        self.pool.with_conn(|conn| metadata::load(conn, id))
    }

    pub fn delete_metadata(&self, id: &str) -> Result<()> {
        self.pool.with_conn(|conn| metadata::delete(conn, id))
    }

    pub fn exists(&self, id: &str) -> Result<bool> {
        self.pool.with_conn(|conn| metadata::exists(conn, id))
    }

    pub fn list_by_source(&self, source: MetadataSource) -> Result<Vec<String>> {
        self.pool
            .with_conn(|conn| metadata::list_ids_by_source(conn, source))
    }

    pub fn get_by_external_id(
        &self,
        source: MetadataSource,
        external_id: &str,
    ) -> Result<Option<ProductMetadata>> {
        self.pool
            .with_conn(|conn| metadata::get_by_external_id(conn, source, external_id))
    }
}
