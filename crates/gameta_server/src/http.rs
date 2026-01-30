//! HTTP API routes
//!
//! REST API endpoints:
//! - GET /api/v1/metadata/:source/:id - Get metadata for a product
//! - POST /api/v1/fetch - Request metadata fetch
//! - GET /api/v1/search?q=... - Search for products
//! - GET /api/v1/health - Health check
//! - POST /api/v1/backup/export - Export database and cache backup
//! - POST /api/v1/backup/import - Import backup
//! - GET /api/docs - Swagger UI

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use gameta_core::MetadataSource;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;

use crate::backup::{self, BackupManifest, ImportReport};
use crate::service::MetadataService;

/// OpenAPI documentation
#[derive(OpenApi)]
#[openapi(
    paths(
        health_check,
        get_metadata,
        fetch_metadata,
        search,
        export_backup,
        import_backup,
    ),
    components(
        schemas(
            HealthResponse,
            FetchRequest,
            FetchResponse,
            SearchQuery,
            SearchResponse,
            MetadataResponse,
            ErrorResponse,
            BackupRequest,
            BackupResponse,
            ImportRequest,
            ImportResponse,
        )
    ),
    tags(
        (name = "health", description = "Health check endpoints"),
        (name = "metadata", description = "Metadata retrieval endpoints"),
        (name = "search", description = "Search endpoints"),
        (name = "backup", description = "Backup and restore endpoints"),
    ),
    info(
        title = "Gameta Server API",
        version = "0.1.0",
        description = "REST API for game metadata storage and retrieval"
    )
)]
pub struct ApiDoc;

/// Create the API router
pub fn create_router(service: Arc<MetadataService>) -> Router {
    Router::new()
        .merge(SwaggerUi::new("/api/docs").url("/api/openapi.json", ApiDoc::openapi()))
        .route("/api/v1/health", get(health_check))
        .route("/api/v1/metadata/{source}/{id}", get(get_metadata))
        .route("/api/v1/fetch", post(fetch_metadata))
        .route("/api/v1/search", get(search))
        .route("/api/v1/backup/export", post(export_backup))
        .route("/api/v1/backup/import", post(import_backup))
        .with_state(service)
}

/// Health check response
#[derive(Serialize, ToSchema)]
pub struct HealthResponse {
    /// Service status
    status: &'static str,
    /// Server version
    version: &'static str,
}

/// Health check endpoint
#[utoipa::path(
    get,
    path = "/api/v1/health",
    tag = "health",
    responses(
        (status = 200, description = "Service is healthy", body = HealthResponse)
    )
)]
async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// Metadata response
#[derive(Serialize, ToSchema)]
pub struct MetadataResponse {
    /// Product ID
    id: String,
    /// Metadata source (dlsite, steam, etc.)
    source: String,
    /// Product title
    title: Option<String>,
    /// Creator/circle name
    creator: Option<String>,
    /// Product description
    description: Option<String>,
    /// Release date
    release_date: Option<String>,
    /// Tags/genres
    tags: Vec<String>,
    /// Additional metadata as JSON
    extras: serde_json::Value,
}

impl From<gameta_core::ProductMetadata> for MetadataResponse {
    fn from(meta: gameta_core::ProductMetadata) -> Self {
        Self {
            id: meta.id,
            source: meta.source.as_str().to_string(),
            title: meta.title,
            creator: meta.creator,
            description: meta.description,
            release_date: meta.release_date,
            tags: meta.tags,
            extras: meta.extras,
        }
    }
}

/// Error response
#[derive(Serialize, ToSchema)]
pub struct ErrorResponse {
    /// Error message
    error: String,
    /// Additional details
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<String>,
}

/// Parse source string to MetadataSource
fn parse_source(source: &str) -> Option<MetadataSource> {
    match source.to_lowercase().as_str() {
        "dlsite" => Some(MetadataSource::DLSite),
        "steam" => Some(MetadataSource::Steam),
        "itchio" | "itch" => Some(MetadataSource::Itchio),
        "gog" => Some(MetadataSource::GOG),
        "custom" => Some(MetadataSource::Custom),
        _ => None,
    }
}

/// Get metadata for a product
#[utoipa::path(
    get,
    path = "/api/v1/metadata/{source}/{id}",
    tag = "metadata",
    params(
        ("source" = String, Path, description = "Metadata source (dlsite, steam, etc.)"),
        ("id" = String, Path, description = "Product ID (e.g., RJ123456)")
    ),
    responses(
        (status = 200, description = "Metadata found", body = MetadataResponse),
        (status = 404, description = "Metadata not found", body = ErrorResponse),
        (status = 400, description = "Invalid source", body = ErrorResponse)
    )
)]
async fn get_metadata(
    State(service): State<Arc<MetadataService>>,
    Path((source, id)): Path<(String, String)>,
) -> impl IntoResponse {
    // Parse source
    let metadata_source = match parse_source(&source) {
        Some(s) => s,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "Invalid source".to_string(),
                    details: Some(format!("Unknown source: {}", source)),
                }),
            ).into_response();
        }
    };

    // Get metadata from service
    match service.get_metadata(metadata_source, &id).await {
        Ok(Some(meta)) => {
            (StatusCode::OK, Json(MetadataResponse::from(meta))).into_response()
        }
        Ok(None) => {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "Metadata not found".to_string(),
                    details: Some(format!("No cached metadata for {}:{}", source, id)),
                }),
            ).into_response()
        }
        Err(e) => {
            tracing::error!("Failed to get metadata: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Internal error".to_string(),
                    details: Some(e.to_string()),
                }),
            ).into_response()
        }
    }
}

/// Fetch request body
#[derive(Deserialize, ToSchema)]
pub struct FetchRequest {
    /// Metadata source (dlsite, steam, etc.)
    source: String,
    /// Product ID
    id: String,
    /// Force refetch even if cached
    #[serde(default)]
    force: bool,
}

/// Fetch response
#[derive(Serialize, ToSchema)]
pub struct FetchResponse {
    /// Request status
    status: String,
    /// Metadata source
    source: String,
    /// Product ID
    id: String,
    /// Fetched metadata (if successful)
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<MetadataResponse>,
}

/// Request metadata fetch
#[utoipa::path(
    post,
    path = "/api/v1/fetch",
    tag = "metadata",
    request_body = FetchRequest,
    responses(
        (status = 200, description = "Fetch successful", body = FetchResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse),
        (status = 500, description = "Fetch failed", body = ErrorResponse)
    )
)]
async fn fetch_metadata(
    State(service): State<Arc<MetadataService>>,
    Json(request): Json<FetchRequest>,
) -> impl IntoResponse {
    // Parse source
    let metadata_source = match parse_source(&request.source) {
        Some(s) => s,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "Invalid source".to_string(),
                    details: Some(format!("Unknown source: {}", request.source)),
                }),
            ).into_response();
        }
    };

    // Fetch metadata
    match service.fetch_metadata(metadata_source, &request.id, request.force).await {
        Ok(meta) => {
            (
                StatusCode::OK,
                Json(FetchResponse {
                    status: "success".to_string(),
                    source: request.source,
                    id: request.id,
                    metadata: Some(MetadataResponse::from(meta)),
                }),
            ).into_response()
        }
        Err(e) => {
            tracing::error!("Failed to fetch metadata: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Fetch failed".to_string(),
                    details: Some(e.to_string()),
                }),
            ).into_response()
        }
    }
}

/// Search query parameters
#[derive(Deserialize, ToSchema)]
pub struct SearchQuery {
    /// Search query string
    q: String,
    /// Optional source filter
    #[serde(default)]
    source: Option<String>,
    /// Maximum results to return
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    20
}

/// Search response
#[derive(Serialize, ToSchema)]
pub struct SearchResponse {
    /// Search query
    query: String,
    /// Source filter (if any)
    source: Option<String>,
    /// Search results
    results: Vec<SearchResultItem>,
}

/// Individual search result
#[derive(Serialize, ToSchema)]
pub struct SearchResultItem {
    /// Product ID
    id: String,
    /// Metadata source
    source: String,
    /// Product title
    title: String,
    /// Creator name
    creator: Option<String>,
    /// Thumbnail URL
    thumbnail_url: Option<String>,
}

impl SearchResultItem {
    /// Convert from core SearchResult with explicit source
    fn from_search_result(r: gameta_core::SearchResult, source: &str) -> Self {
        Self {
            id: r.external_id,
            source: source.to_string(),
            title: r.title,
            creator: r.creator,
            thumbnail_url: r.thumbnail_url,
        }
    }
}

/// Search for products
#[utoipa::path(
    get,
    path = "/api/v1/search",
    tag = "search",
    params(
        ("q" = String, Query, description = "Search query"),
        ("source" = Option<String>, Query, description = "Filter by source"),
        ("limit" = Option<usize>, Query, description = "Maximum results (default: 20)")
    ),
    responses(
        (status = 200, description = "Search results", body = SearchResponse),
        (status = 400, description = "Invalid request", body = ErrorResponse)
    )
)]
async fn search(
    State(service): State<Arc<MetadataService>>,
    Query(query): Query<SearchQuery>,
) -> impl IntoResponse {
    // Parse optional source filter
    let source_filter = query.source.as_ref().and_then(|s| parse_source(s));

    // Perform search
    let search_source = query.source.as_deref().unwrap_or("dlsite");
    match service.search(&query.q, source_filter).await {
        Ok(results) => {
            let items: Vec<SearchResultItem> = results
                .into_iter()
                .take(query.limit)
                .map(|r| SearchResultItem::from_search_result(r, search_source))
                .collect();

            (
                StatusCode::OK,
                Json(SearchResponse {
                    query: query.q,
                    source: query.source,
                    results: items,
                }),
            ).into_response()
        }
        Err(e) => {
            tracing::error!("Search failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Search failed".to_string(),
                    details: Some(e.to_string()),
                }),
            ).into_response()
        }
    }
}

/// Backup request body
#[derive(Deserialize, ToSchema)]
pub struct BackupRequest {
    /// Output file path for the backup
    #[serde(default = "default_backup_path")]
    output_path: String,
}

fn default_backup_path() -> String {
    format!("backup_{}.tar.gz", chrono::Utc::now().format("%Y%m%d_%H%M%S"))
}

/// Backup response
#[derive(Serialize, ToSchema)]
pub struct BackupResponse {
    /// Whether backup was successful
    success: bool,
    /// Path to the backup file
    output_path: String,
    /// Backup manifest with metadata
    manifest: BackupManifestResponse,
}

/// Backup manifest (API response version)
#[derive(Serialize, ToSchema)]
pub struct BackupManifestResponse {
    /// Backup format version
    version: String,
    /// When the backup was created
    created_at: String,
    /// Number of metadata entries
    metadata_count: u64,
    /// Number of cached content items
    content_count: u64,
    /// Total size in bytes
    total_size: u64,
}

impl From<BackupManifest> for BackupManifestResponse {
    fn from(m: BackupManifest) -> Self {
        Self {
            version: m.version,
            created_at: m.created_at.to_rfc3339(),
            metadata_count: m.metadata_count,
            content_count: m.content_count,
            total_size: m.database_size + m.cache_size,
        }
    }
}

/// Export backup
#[utoipa::path(
    post,
    path = "/api/v1/backup/export",
    tag = "backup",
    request_body = BackupRequest,
    responses(
        (status = 200, description = "Backup created successfully", body = BackupResponse),
        (status = 500, description = "Backup failed", body = ErrorResponse)
    )
)]
async fn export_backup(
    State(service): State<Arc<MetadataService>>,
    Json(request): Json<BackupRequest>,
) -> impl IntoResponse {
    let config = service.config();
    let db_path = &config.database_path;
    let cache_dir = &config.cache_dir;
    let output_path = std::path::PathBuf::from(&request.output_path);

    match backup::export_backup(db_path, cache_dir, &output_path).await {
        Ok(manifest) => {
            (
                StatusCode::OK,
                Json(BackupResponse {
                    success: true,
                    output_path: request.output_path,
                    manifest: BackupManifestResponse::from(manifest),
                }),
            ).into_response()
        }
        Err(e) => {
            tracing::error!("Backup export failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Backup failed".to_string(),
                    details: Some(e.to_string()),
                }),
            ).into_response()
        }
    }
}

/// Import request body
#[derive(Deserialize, ToSchema)]
pub struct ImportRequest {
    /// Path to the backup file to import
    backup_path: String,
}

/// Import response
#[derive(Serialize, ToSchema)]
pub struct ImportResponse {
    /// Whether import was successful
    success: bool,
    /// Number of metadata entries imported
    metadata_imported: u64,
    /// Number of content items imported
    content_imported: u64,
    /// Any warnings during import
    warnings: Vec<String>,
}

impl From<ImportReport> for ImportResponse {
    fn from(r: ImportReport) -> Self {
        Self {
            success: r.success,
            metadata_imported: r.metadata_imported,
            content_imported: r.content_imported,
            warnings: r.warnings,
        }
    }
}

/// Import backup
#[utoipa::path(
    post,
    path = "/api/v1/backup/import",
    tag = "backup",
    request_body = ImportRequest,
    responses(
        (status = 200, description = "Backup imported successfully", body = ImportResponse),
        (status = 400, description = "Invalid backup file", body = ErrorResponse),
        (status = 500, description = "Import failed", body = ErrorResponse)
    )
)]
async fn import_backup(
    State(service): State<Arc<MetadataService>>,
    Json(request): Json<ImportRequest>,
) -> impl IntoResponse {
    let config = service.config();
    let db_path = &config.database_path;
    let cache_dir = &config.cache_dir;
    let backup_path = std::path::PathBuf::from(&request.backup_path);

    // Check if backup file exists
    if !backup_path.exists() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Backup file not found".to_string(),
                details: Some(format!("File not found: {}", request.backup_path)),
            }),
        ).into_response();
    }

    match backup::import_backup(&backup_path, db_path, cache_dir).await {
        Ok(report) => {
            (
                StatusCode::OK,
                Json(ImportResponse::from(report)),
            ).into_response()
        }
        Err(e) => {
            tracing::error!("Backup import failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Import failed".to_string(),
                    details: Some(e.to_string()),
                }),
            ).into_response()
        }
    }
}
