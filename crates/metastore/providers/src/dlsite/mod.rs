//! DLSite metadata provider

mod detect;
mod parse;

use crate::{MetadataProvider, ParseError};
use metastore_abstract::{HttpRequest, HttpResponse};
use metastore_types::{MetadataSource, ProductMetadata, SearchResult};

pub use detect::detect_dlsite_code;
pub use parse::{parse_api_response, parse_html_response, parse_search_response};

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
        vec![
            HttpRequest::get(
                &format!(
                    "https://www.dlsite.com/home/api/=/product.json?work_no={}",
                    external_id
                ),
                &format!("dlsite:json:{}", external_id),
            ),
            HttpRequest::get(
                &format!(
                    "https://www.dlsite.com/home/work/=/product_id/{}.html",
                    external_id
                ),
                &format!("dlsite:html:{}", external_id),
            ),
        ]
    }

    fn parse_responses(
        &self,
        external_id: &str,
        responses: &[(&str, HttpResponse)],
    ) -> Result<ProductMetadata, ParseError> {
        // Find JSON and HTML responses
        let json_response = responses
            .iter()
            .find(|(key, _)| key.contains(":json:"))
            .map(|(_, r)| r);
        let html_response = responses
            .iter()
            .find(|(key, _)| key.contains(":html:"))
            .map(|(_, r)| r);

        // Parse JSON API response
        let mut meta = if let Some(resp) = json_response {
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
