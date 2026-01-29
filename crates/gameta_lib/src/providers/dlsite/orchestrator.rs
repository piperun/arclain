use super::api;
use super::options::DlsiteFetchOptions;

/// A step in the metadata fetching process
#[derive(Debug, PartialEq, Eq)]
pub enum FetchStep {
    /// Fetch from the JSON API
    FetchJson(String),
    /// Scrape the HTML page
    FetchHtml(String),
}

/// Helper to generate the fetch plan based on options.
pub fn plan_fetch(product_id: &str, options: DlsiteFetchOptions) -> Vec<FetchStep> {
    let mut steps = Vec::new();

    if options.contains(DlsiteFetchOptions::JSON_API) {
        steps.push(FetchStep::FetchJson(api::ajax_url(product_id)));
    }

    if options.contains(DlsiteFetchOptions::HTML_SCRAPE) {
        steps.push(FetchStep::FetchHtml(api::html_url(product_id)));
    }

    steps
}
