//! Fetch logging for rate limiting
//!
//! Tracks all external HTTP requests to prevent getting banned from
//! metadata sources (especially DLSite which is sensitive to scraping).

use gameta_core::StorageError;
use libsql::params;

use super::helpers::chrono_lite_now;
use super::LibSqlBackend;

impl LibSqlBackend {
    /// Log a fetch request
    ///
    /// Records every external HTTP request for rate limiting and analytics.
    ///
    /// # Arguments
    /// * `source` - The metadata source (e.g., "dlsite", "steam")
    /// * `endpoint` - Type of endpoint (e.g., "api", "html", "image", "search")
    /// * `product_id` - Optional product ID being fetched
    /// * `response_status` - HTTP response status code
    /// * `response_size` - Response body size in bytes
    /// * `cached` - Whether this was served from cache
    pub async fn log_fetch(
        &self,
        source: &str,
        endpoint: &str,
        product_id: Option<&str>,
        response_status: Option<i32>,
        response_size: Option<i64>,
        cached: bool,
    ) -> Result<(), StorageError> {
        let now = chrono_lite_now();
        self.conn
            .execute(
                "INSERT INTO fetch_log (source, endpoint, product_id, requested_at, response_status, response_size, cached)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![source, endpoint, product_id, now, response_status, response_size, cached as i32],
            )
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?;
        Ok(())
    }

    /// Get the timestamp of the last fetch for a source
    ///
    /// Useful for implementing minimum delays between requests.
    pub async fn last_fetch_time(&self, source: &str) -> Result<Option<String>, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT requested_at FROM fetch_log WHERE source = ?1 ORDER BY requested_at DESC LIMIT 1",
                params![source],
            )
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?;

        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?
        {
            Ok(row.get(0).ok())
        } else {
            Ok(None)
        }
    }

    /// Count recent fetches for rate limiting
    ///
    /// # Arguments
    /// * `source` - The metadata source to check
    /// * `since` - Timestamp (seconds since epoch) to count from
    ///
    /// # Example
    /// ```ignore
    /// // Check if we've made more than 10 requests in the last minute
    /// let one_minute_ago = (SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() - 60).to_string();
    /// let count = backend.count_recent_fetches("dlsite", &one_minute_ago).await?;
    /// if count > 10 {
    ///     // Too many requests, wait before making more
    /// }
    /// ```
    pub async fn count_recent_fetches(&self, source: &str, since: &str) -> Result<i64, StorageError> {
        let mut rows = self
            .conn
            .query(
                "SELECT COUNT(*) FROM fetch_log WHERE source = ?1 AND requested_at >= ?2",
                params![source, since],
            )
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?;

        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?
        {
            Ok(row.get(0).unwrap_or(0))
        } else {
            Ok(0)
        }
    }
}
