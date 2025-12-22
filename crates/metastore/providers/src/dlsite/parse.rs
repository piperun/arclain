//! DLSite response parsing

use crate::ParseError;
use metastore_types::{MetadataSource, ProductMetadata, SearchResult};
use scraper::{Html, Selector};

/// Parse DLSite API JSON response
pub fn parse_api_response(
    external_id: &str,
    json_str: &str,
) -> Result<ProductMetadata, ParseError> {
    let json: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| ParseError::InvalidFormat(format!("Invalid JSON: {}", e)))?;

    // API returns array, get first item
    let data = if let Some(arr) = json.as_array() {
        arr.first().cloned().unwrap_or(json.clone())
    } else {
        json
    };

    let mut meta = ProductMetadata::new(MetadataSource::DLSite, external_id);

    // Extract fields from JSON
    meta.title = data["work_name"].as_str().map(|s| s.to_string());
    meta.creator = data["maker_name"].as_str().map(|s| s.to_string());
    meta.release_date = data["regist_date"].as_str().map(|s| s.to_string());

    // Price
    meta.price = data["price"]
        .as_i64()
        .or_else(|| data["price"].as_str().and_then(|s| s.parse().ok()));
    meta.currency = Some("JPY".to_string());

    // Stats
    meta.rating = data["rate_average_2dp"].as_f64();
    meta.rating_count = data["rate_count"].as_i64();
    meta.purchase_count = data["dl_count"].as_i64();
    meta.favorite_count = data["wishlist_count"].as_i64();
    meta.review_count = data["review_count"].as_i64();

    // File info
    meta.file_size = data["file_size"].as_str().map(|s| s.to_string());
    meta.file_format = data["file_type"].as_str().map(|s| s.to_string());
    meta.age_rating = data["age_category_string"].as_str().map(|s| s.to_string());

    // Genres/tags from API
    if let Some(genres) = data["genre"].as_array() {
        meta.genres = genres
            .iter()
            .filter_map(|g| g["name"].as_str().map(|s| s.to_string()))
            .collect();
    }

    // Store raw response
    meta.raw_api_response = Some(json_str.to_string());

    Ok(meta)
}

/// Scraped data from HTML
pub struct ScrapedData {
    pub title: Option<String>,
    pub circle: Option<String>,
    pub release_date: Option<String>,
    pub tags: Vec<String>,
    pub description: Option<String>,
    pub cover_image: Option<String>,
    pub screenshots: Vec<String>,
}

/// Parse DLSite HTML page for additional data
pub fn parse_html_response(html: &str) -> Option<ScrapedData> {
    let document = Html::parse_document(html);

    let mut data = ScrapedData {
        title: None,
        circle: None,
        release_date: None,
        tags: Vec::new(),
        description: None,
        cover_image: None,
        screenshots: Vec::new(),
    };

    // Title
    if let Ok(sel) = Selector::parse("h1#work_name") {
        if let Some(el) = document.select(&sel).next() {
            data.title = Some(el.text().collect::<String>().trim().to_string());
        }
    }

    // Circle/maker
    if let Ok(sel) = Selector::parse("span.maker_name a") {
        if let Some(el) = document.select(&sel).next() {
            data.circle = Some(el.text().collect::<String>().trim().to_string());
        }
    }

    // Description
    if let Ok(sel) = Selector::parse("div.work_parts_container") {
        if let Some(el) = document.select(&sel).next() {
            data.description = Some(el.text().collect::<String>().trim().to_string());
        }
    }

    // Tags
    if let Ok(sel) = Selector::parse("div.main_genre a") {
        data.tags = document
            .select(&sel)
            .map(|el| el.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }

    // Cover image
    if let Ok(sel) = Selector::parse("div.product-slider-data div") {
        if let Some(el) = document.select(&sel).next() {
            if let Some(src) = el.value().attr("data-src") {
                data.cover_image = Some(format!("https:{}", src));
            }
        }
    }

    // Screenshots
    if let Ok(sel) = Selector::parse("div.product-slider-data div[data-src]") {
        data.screenshots = document
            .select(&sel)
            .skip(1) // First is usually cover
            .filter_map(|el| el.value().attr("data-src"))
            .map(|src| format!("https:{}", src))
            .collect();
    }

    Some(data)
}

/// Parse DLSite search results page
pub fn parse_search_response(html: &str) -> Vec<SearchResult> {
    use super::detect_dlsite_code;

    let document = Html::parse_document(html);
    let mut results = Vec::new();

    // Select search results - try multiple selectors as DLSite layout might vary
    let item_selector = Selector::parse("li.search_result_img_box_inner, tr.n_worklist_item")
        .unwrap_or_else(|_| Selector::parse("div").unwrap());
    let title_selector = Selector::parse("dt.work_name a, a.work_name")
        .unwrap_or_else(|_| Selector::parse("a").unwrap());
    let maker_selector = Selector::parse("dd.maker_name a, span.maker_name a")
        .unwrap_or_else(|_| Selector::parse("span").unwrap());

    for item in document.select(&item_selector) {
        let mut title = "Unknown".to_string();
        let mut creator = None;
        let mut code = String::new();
        let mut thumbnail = None;

        if let Some(link) = item.select(&title_selector).next() {
            title = link.text().collect::<String>().trim().to_string();
            if let Some(href) = link.value().attr("href") {
                // Extract code from URL (.../product_id/RJ123456.html)
                if let Some(c) = detect_dlsite_code(href) {
                    code = c;
                }
            }
        }

        if let Some(maker_link) = item.select(&maker_selector).next() {
            creator = Some(maker_link.text().collect::<String>().trim().to_string());
        }

        // Try to find thumbnail
        if let Ok(img_sel) = Selector::parse("img") {
            if let Some(img) = item.select(&img_sel).next() {
                if let Some(src) = img
                    .value()
                    .attr("src")
                    .or_else(|| img.value().attr("data-src"))
                {
                    thumbnail = Some(if src.starts_with("//") {
                        format!("https:{}", src)
                    } else {
                        src.to_string()
                    });
                }
            }
        }

        if !code.is_empty() {
            results.push(SearchResult {
                external_id: code,
                title,
                creator,
                thumbnail_url: thumbnail,
            });
        }

        if results.len() >= 20 {
            break;
        }
    }

    results
}
