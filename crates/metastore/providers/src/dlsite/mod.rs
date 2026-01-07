//! DLSite metadata provider

pub mod api;
pub mod cache_keys;
mod detect;
mod parse;
#[cfg(test)]
mod tests;

use crate::{MetadataProvider, ParseError};
use metastore_abstract::{HttpRequest, HttpResponse};
use metastore_types::{MetadataSource, ProductMetadata, SearchResult};

pub use api::{ajax_url, get_site_id, html_url, is_geo_blocked, ConnectivityResult};
pub use detect::detect_dlsite_code;
pub use parse::{parse_api_response, parse_html_response, parse_search_response, ScrapedData};

/// DLSite metadata provider
pub struct DLSiteProvider;

impl Default for DLSiteProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl DLSiteProvider {
    pub fn new() -> Self {
        Self
    }
}

impl MetadataProvider for DLSiteProvider {
    fn id(&self) -> MetadataSource {
        MetadataSource::DLSite
    }

    fn detect(&self, text: &str) -> Option<String> {
        detect_dlsite_code(text)
    }

    fn request_metadata(&self, external_id: &str) -> Vec<HttpRequest> {
        // Use the api module for URL construction
        let mut ajax_req = HttpRequest::get(
            &api::ajax_url(external_id),
            &format!("dlsite:ajax:{}", external_id),
        );
        ajax_req
            .headers
            .insert("Cookie".to_string(), "adultchecked=1".to_string());
        ajax_req.headers.insert("User-Agent".to_string(), "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36".to_string());

        let mut html_req = HttpRequest::get(
            &api::html_url(external_id),
            &format!("dlsite:html:{}", external_id),
        );
        html_req
            .headers
            .insert("Cookie".to_string(), "adultchecked=1".to_string());
        html_req.headers.insert("User-Agent".to_string(), "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36".to_string());

        vec![ajax_req, html_req]
    }

    fn parse_responses(
        &self,
        external_id: &str,
        responses: &[(&str, HttpResponse)],
    ) -> Result<ProductMetadata, ParseError> {
        // Find AJAX and HTML responses
        let ajax_response = responses
            .iter()
            .find(|(key, _)| key.contains(":ajax:"))
            .map(|(_, r)| r);
        let html_response = responses
            .iter()
            .find(|(key, _)| key.contains(":html:"))
            .map(|(_, r)| r);

        // Parse AJAX API response
        let mut meta = if let Some(resp) = ajax_response {
            let body = resp
                .body_str()
                .map_err(|e| ParseError::InvalidFormat(format!("UTF-8 error: {}", e)))?;
            parse_api_response(external_id, body)?
        } else {
            ProductMetadata::new(MetadataSource::DLSite, external_id)
        };

        // Augment with HTML scraping
        if let Some(resp) = html_response {
            if let Ok(body) = resp.body_str() {
                if let Some(scraped) = parse_html_response(body) {
                    // Merge scraped data
                    if meta.title.is_none() {
                        meta.title = scraped.title;
                    }
                    if meta.creator.is_none() {
                        meta.creator = scraped.circle;
                    }
                    if meta.description.is_none() {
                        meta.description = scraped.description;
                    }
                    if meta.tags.is_empty() {
                        meta.tags = scraped.tags;
                    }
                    // Store raw HTML
                    meta.raw_html = Some(body.to_string());
                }
            }
        }

        Ok(meta)
    }

    fn request_search(&self, query: &str) -> HttpRequest {
        HttpRequest::get(
            &format!(
                "https://www.dlsite.com/home/fsr/=/language/jp/keyword/{}/order/trend/per_page/30/page/1",
                urlencoding::encode(query)
            ),
            &format!("dlsite:search:{}", query),
        )
    }

    fn parse_search(&self, response: &HttpResponse) -> Result<Vec<SearchResult>, ParseError> {
        let body = response
            .body_str()
            .map_err(|e| ParseError::InvalidFormat(format!("UTF-8 error: {}", e)))?;
        Ok(parse_search_response(body))
    }
}
