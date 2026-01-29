//! HTTP API routes
//!
//! REST API endpoints:
//! - GET /api/v1/metadata/:source/:id - Get metadata for a product
//! - POST /api/v1/fetch - Request metadata fetch
//! - GET /api/v1/search?q=... - Search for products
//! - GET /api/v1/health - Health check

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::service::MetadataService;

/// Create the API router
pub fn create_router(service: Arc<MetadataService>) -> Router {
    Router::new()
        .route("/api/v1/health", get(health_check))
        .route("/api/v1/metadata/:source/:id", get(get_metadata))
        .route("/api/v1/fetch", post(fetch_metadata))
        .route("/api/v1/search", get(search))
        .with_state(service)
}

/// Health check response
#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// Get metadata for a product
async fn get_metadata(
    State(_service): State<Arc<MetadataService>>,
    Path((source, id)): Path<(String, String)>,
) -> impl IntoResponse {
    // TODO: Implement
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "error": "Not implemented",
            "source": source,
            "id": id
        })),
    )
}

/// Fetch request body
#[derive(Deserialize)]
struct FetchRequest {
    source: String,
    id: String,
    #[serde(default)]
    force: bool,
}

/// Request metadata fetch
async fn fetch_metadata(
    State(_service): State<Arc<MetadataService>>,
    Json(request): Json<FetchRequest>,
) -> impl IntoResponse {
    // TODO: Implement
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "status": "queued",
            "source": request.source,
            "id": request.id
        })),
    )
}

/// Search query parameters
#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    20
}

/// Search for products
async fn search(
    State(_service): State<Arc<MetadataService>>,
    Query(query): Query<SearchQuery>,
) -> impl IntoResponse {
    // TODO: Implement
    Json(serde_json::json!({
        "query": query.q,
        "source": query.source,
        "results": []
    }))
}
