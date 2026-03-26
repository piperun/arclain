//! HTTP client for the gameta metadata server API.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Connection configuration for a gameta server instance.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Base URL of the gameta server (e.g. `http://localhost:8080`).
    pub url: String,
    /// Optional API key sent as a Bearer token.
    pub api_key: Option<String>,
}

/// Response from `GET /api/v1/health`.
#[derive(Debug, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

/// Full metadata record returned by the server.
#[derive(Debug, Serialize, Deserialize)]
pub struct MetadataResponse {
    pub id: String,
    pub source: String,
    pub title: Option<String>,
    pub creator: Option<String>,
    pub description: Option<String>,
    pub release_date: Option<String>,
    pub tags: Vec<String>,
    pub extras: serde_json::Value,
}

/// Body sent to `POST /api/v1/fetch`.
#[derive(Debug, Serialize)]
pub struct FetchRequest {
    pub source: String,
    pub id: String,
    pub force: bool,
}

/// Response from `POST /api/v1/fetch`.
#[derive(Debug, Deserialize)]
pub struct FetchResponse {
    pub status: String,
    pub source: String,
    pub id: String,
    pub metadata: Option<MetadataResponse>,
}

/// A single item in a search result list.
#[derive(Debug, Deserialize)]
pub struct SearchResultItem {
    pub id: String,
    pub source: String,
    pub title: String,
    pub creator: Option<String>,
    pub thumbnail_url: Option<String>,
}

/// Response from `GET /api/v1/search`.
#[derive(Debug, Deserialize)]
pub struct SearchResponse {
    pub query: String,
    pub source: Option<String>,
    pub results: Vec<SearchResultItem>,
}

/// Error body returned by the server on non-2xx responses.
#[derive(Debug, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: Option<String>,
}

// ---------------------------------------------------------------------------

/// Blocking HTTP client for the gameta server API.
///
/// Uses `reqwest::blocking::Client` so it can be called from synchronous
/// plugin host functions without needing an async runtime on the call stack.
pub struct GametaClient {
    config: ServerConfig,
    client: reqwest::blocking::Client,
    /// Server version string captured from the most recent successful health
    /// check. `None` if a health check has not yet succeeded.
    server_version: std::sync::RwLock<Option<String>>,
}

impl GametaClient {
    /// Build a new client from `config`. Times out requests after 10 seconds.
    pub fn new(config: ServerConfig) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to build GametaClient HTTP client");

        Self {
            config,
            client,
            server_version: std::sync::RwLock::new(None),
        }
    }

    /// Return the server version captured from the last successful health
    /// check, if any.
    pub fn last_known_version(&self) -> Option<String> {
        self.server_version
            .read()
            .ok()
            .and_then(|v| v.clone())
    }

    /// Join the server's base URL (trailing slash stripped) with `path`.
    pub fn endpoint(&self, path: &str) -> String {
        let base = self.config.url.trim_end_matches('/');
        format!("{}{}", base, path)
    }

    /// Add an `Authorization: Bearer` header if an API key is configured.
    fn auth(
        &self,
        req: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        if let Some(key) = &self.config.api_key {
            req.bearer_auth(key)
        } else {
            req
        }
    }

    /// `GET /api/v1/health` — no authentication required.
    ///
    /// On success the server's version string is cached and retrievable via
    /// [`GametaClient::last_known_version`].
    pub fn health(&self) -> Result<HealthResponse, String> {
        let url = self.endpoint("/api/v1/health");
        let resp = self
            .client
            .get(&url)
            .send()
            .map_err(|e| format!("Health request failed: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("Health check returned HTTP {}", resp.status()));
        }

        let health = resp
            .json::<HealthResponse>()
            .map_err(|e| format!("Failed to parse health response: {}", e))?;

        // Cache the version for callers that don't have access to this response.
        if let Ok(mut v) = self.server_version.write() {
            *v = Some(health.version.clone());
        }

        Ok(health)
    }

    /// `GET /api/v1/metadata/{source}/{id}` — returns `None` on 404.
    pub fn get_metadata(
        &self,
        source: &str,
        id: &str,
    ) -> Result<Option<MetadataResponse>, String> {
        let url = self.endpoint(&format!(
            "/api/v1/metadata/{}/{}",
            urlencoding::encode(source),
            urlencoding::encode(id),
        ));

        let resp = self
            .auth(self.client.get(&url))
            .send()
            .map_err(|e| format!("get_metadata request failed: {}", e))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !resp.status().is_success() {
            let code = resp.status();
            let body = resp
                .json::<ErrorResponse>()
                .map(|e| e.error)
                .unwrap_or_else(|_| format!("HTTP {}", code));
            return Err(body);
        }

        resp.json::<MetadataResponse>()
            .map(Some)
            .map_err(|e| format!("Failed to parse metadata response: {}", e))
    }

    /// `POST /api/v1/fetch` — ask the server to fetch/refresh metadata.
    pub fn fetch_metadata(
        &self,
        source: &str,
        id: &str,
        force: bool,
    ) -> Result<FetchResponse, String> {
        let url = self.endpoint("/api/v1/fetch");
        let body = FetchRequest {
            source: source.to_string(),
            id: id.to_string(),
            force,
        };

        let resp = self
            .auth(self.client.post(&url))
            .json(&body)
            .send()
            .map_err(|e| format!("fetch_metadata request failed: {}", e))?;

        if !resp.status().is_success() {
            let code = resp.status();
            let msg = resp
                .json::<ErrorResponse>()
                .map(|e| e.error)
                .unwrap_or_else(|_| format!("HTTP {}", code));
            return Err(msg);
        }

        resp.json::<FetchResponse>()
            .map_err(|e| format!("Failed to parse fetch response: {}", e))
    }

    /// `GET /api/v1/search?q=...&source=...&limit=...`
    pub fn search(
        &self,
        query: &str,
        source: Option<&str>,
        limit: Option<u32>,
    ) -> Result<SearchResponse, String> {
        let url = self.endpoint("/api/v1/search");
        let mut req = self.auth(self.client.get(&url)).query(&[("q", query)]);

        if let Some(src) = source {
            req = req.query(&[("source", src)]);
        }
        if let Some(lim) = limit {
            req = req.query(&[("limit", lim.to_string().as_str())]);
        }

        let resp = req
            .send()
            .map_err(|e| format!("search request failed: {}", e))?;

        if !resp.status().is_success() {
            let code = resp.status();
            let msg = resp
                .json::<ErrorResponse>()
                .map(|e| e.error)
                .unwrap_or_else(|_| format!("HTTP {}", code));
            return Err(msg);
        }

        resp.json::<SearchResponse>()
            .map_err(|e| format!("Failed to parse search response: {}", e))
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_health_response_parsing() {
        let raw = json!({ "status": "ok", "version": "1.2.3" });
        let parsed: HealthResponse =
            serde_json::from_value(raw).expect("should parse HealthResponse");
        assert_eq!(parsed.status, "ok");
        assert_eq!(parsed.version, "1.2.3");
    }

    #[test]
    fn test_server_config_base_url_trailing_slash() {
        let client = GametaClient::new(ServerConfig {
            url: "http://localhost:8080/".to_string(),
            api_key: None,
        });
        assert_eq!(
            client.endpoint("/api/v1/health"),
            "http://localhost:8080/api/v1/health"
        );

        // Also verify no trailing slash on base
        let client2 = GametaClient::new(ServerConfig {
            url: "http://localhost:8080".to_string(),
            api_key: None,
        });
        assert_eq!(
            client2.endpoint("/api/v1/health"),
            "http://localhost:8080/api/v1/health"
        );
    }

    #[test]
    fn test_metadata_response_parsing() {
        let raw = json!({
            "id": "RJ123456",
            "source": "dlsite",
            "title": "Some Title",
            "creator": "Some Circle",
            "description": "A game about things.",
            "release_date": "2024-01-15",
            "tags": ["fantasy", "rpg"],
            "extras": { "price": 1100, "currency": "JPY" }
        });
        let parsed: MetadataResponse =
            serde_json::from_value(raw).expect("should parse MetadataResponse");
        assert_eq!(parsed.id, "RJ123456");
        assert_eq!(parsed.source, "dlsite");
        assert_eq!(parsed.title.as_deref(), Some("Some Title"));
        assert_eq!(parsed.creator.as_deref(), Some("Some Circle"));
        assert_eq!(parsed.tags, vec!["fantasy", "rpg"]);
        assert_eq!(parsed.extras["price"], 1100);
    }

    #[test]
    fn test_error_response_parsing() {
        let raw = json!({ "error": "not found", "code": "E404" });
        let parsed: ErrorResponse =
            serde_json::from_value(raw).expect("should parse ErrorResponse");
        assert_eq!(parsed.error, "not found");
        assert_eq!(parsed.code.as_deref(), Some("E404"));

        // code is optional
        let raw2 = json!({ "error": "internal error" });
        let parsed2: ErrorResponse =
            serde_json::from_value(raw2).expect("should parse ErrorResponse without code");
        assert_eq!(parsed2.error, "internal error");
        assert!(parsed2.code.is_none());
    }
}
