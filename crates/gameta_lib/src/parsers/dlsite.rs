//! DLSite metadata parser
//!
//! Pure parsing functions for DLSite API JSON and HTML responses.
//! No HTTP dependencies - just give it the data and it will parse it.

use crate::types::{MetadataSource, ParseError, ProductMetadata, SearchResult};
use scraper::{Html, Selector};

/// Scraped data from DLSite HTML page
#[derive(Debug, Clone, Default)]
pub struct ScrapedData {
    pub title: Option<String>,
    pub circle: Option<String>,
    pub release_date: Option<String>,
    pub update_date: Option<String>,
    pub tags: Vec<String>,
    pub description: Option<String>,
    pub cover_image: Option<String>,
    pub screenshots: Vec<String>,
    pub voice_actors: Vec<String>,
    pub authors: Vec<String>,
    pub illustrators: Vec<String>,
    pub scenarios: Vec<String>,
    pub musicians: Vec<String>,
    pub writers: Vec<String>,
    pub brand: Option<String>,
    pub publisher: Option<String>,
    pub series: Option<String>,
    pub page_count: Option<i64>,
    pub file_size: Option<String>,
    pub genres: Vec<String>,
    pub geo_blocked: bool,
}

/// Parse DLSite metadata from raw API JSON and/or HTML.
///
/// This is the main entry point. Give it whatever data you have.
///
/// # Arguments
/// * `product_id` - The DLSite product ID (e.g., "RJ123456")
/// * `api_json` - Optional raw JSON string from the ajax API
/// * `html` - Optional raw HTML string from the work page
///
/// # Returns
/// A `ProductMetadata` struct populated with parsed data.
pub fn parse_dlsite(
    product_id: &str,
    api_json: Option<&str>,
    html: Option<&str>,
) -> Result<ProductMetadata, ParseError> {
    let mut meta = if let Some(json) = api_json {
        parse_api_json(product_id, json)?
    } else {
        ProductMetadata::new(MetadataSource::DLSite, product_id)
    };

    if let Some(html_str) = html {
        if let Some(scraped) = parse_html(html_str) {
            merge_scraped_data(&mut meta, &scraped);
        }
    }

    Ok(meta)
}

/// Parse DLSite API JSON response
pub fn parse_api_json(product_id: &str, json_str: &str) -> Result<ProductMetadata, ParseError> {
    let json: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| ParseError::InvalidFormat(format!("Invalid JSON: {}", e)))?;

    // API returns array, get first item
    let data = if let Some(arr) = json.as_array() {
        arr.first().cloned().unwrap_or(json.clone())
    } else {
        json
    };

    let mut meta = ProductMetadata::new(MetadataSource::DLSite, product_id);

    // Extract fields from JSON
    meta.title = data["work_name"].as_str().map(|s| s.to_string());
    meta.creator = data["maker_name"].as_str().map(|s| s.to_string());

    // Clean date: "2023-01-01 00:00:00" -> "2023-01-01"
    meta.release_date = data["regist_date"]
        .as_str()
        .map(|s| s.split_whitespace().next().unwrap_or(s).to_string());

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

    Ok(meta)
}

/// Parse DLSite HTML page for additional data
pub fn parse_html(html: &str) -> Option<ScrapedData> {
    let document = Html::parse_document(html);
    let mut data = ScrapedData::default();

    // Check for geo-blocking
    let html_lower = html.to_lowercase();
    let geo_blocked_patterns = [
        "お住いの国・地域からは本作品は購入できません",
        "this product cannot be purchased",
        "このページはお住まいの地域からは表示できません",
        "this page cannot be displayed",
        "access denied",
        "region restricted",
    ];

    for pattern in geo_blocked_patterns {
        if html_lower.contains(&pattern.to_lowercase()) {
            data.geo_blocked = true;
            break;
        }
    }

    // Also check for missing essential content
    if !data.geo_blocked
        && !html_lower.contains("work_outline")
        && !html_lower.contains("work_name")
    {
        data.geo_blocked = true;
    }

    // Title
    if let Ok(sel) = Selector::parse("h1#work_name") {
        if let Some(el) = document.select(&sel).next() {
            data.title = Some(clean_text(&el.text().collect::<String>()));
        }
    }

    // If geo-blocked, return early with what we have
    if data.geo_blocked {
        return Some(data);
    }

    // Circle/maker
    if let Ok(sel) = Selector::parse("#work_right span.maker_name a, #work_maker span.maker_name a")
    {
        if let Some(el) = document.select(&sel).next() {
            data.circle = Some(clean_text(&el.text().collect::<String>()));
        }
    }

    // Work Maker Table (Brand, Publisher)
    if let Ok(table_sel) = Selector::parse("table#work_maker tr") {
        for row in document.select(&table_sel) {
            let th_sel = Selector::parse("th").unwrap();
            let td_sel = Selector::parse("td").unwrap();

            if let (Some(th), Some(td)) = (row.select(&th_sel).next(), row.select(&td_sel).next()) {
                let header = clean_text(&th.text().collect::<String>());
                let content = clean_text(&td.text().collect::<String>());

                match header.as_str() {
                    "Brand" | "ブランド" => data.brand = Some(content),
                    "Circle" | "サークル" => data.circle = Some(content),
                    "Publisher" | "出版社" => data.publisher = Some(content),
                    _ => {}
                }
            }
        }
    }

    // Work Outline Table
    if let Ok(table_sel) = Selector::parse("table#work_outline tr") {
        for row in document.select(&table_sel) {
            let th_sel = Selector::parse("th").unwrap();
            let td_sel = Selector::parse("td").unwrap();
            let a_sel = Selector::parse("a").unwrap();

            if let (Some(th), Some(td)) = (row.select(&th_sel).next(), row.select(&td_sel).next()) {
                let header = clean_text(&th.text().collect::<String>());

                match header.as_str() {
                    "Release Date" | "販売日" => {
                        data.release_date = Some(clean_text(&td.text().collect::<String>()));
                    }
                    "Update Date" | "更新日" => {
                        data.update_date = Some(clean_text(&td.text().collect::<String>()));
                    }
                    "Series" | "シリーズ名" => {
                        data.series = Some(clean_text(&td.text().collect::<String>()));
                    }
                    "Page Count" | "ページ数" => {
                        let text = clean_text(&td.text().collect::<String>());
                        if let Some(num_str) = text.split_whitespace().next() {
                            data.page_count = num_str.parse().ok();
                        }
                    }
                    "File size" | "ファイル容量" => {
                        data.file_size = Some(clean_text(&td.text().collect::<String>()));
                    }
                    "Voice Actor" | "声優" => {
                        data.voice_actors = td
                            .select(&a_sel)
                            .map(|a| clean_text(&a.text().collect::<String>()))
                            .collect();
                    }
                    "Author" | "作者" => {
                        data.authors = td
                            .select(&a_sel)
                            .map(|a| clean_text(&a.text().collect::<String>()))
                            .collect();
                    }
                    "Illustration" | "イラスト" => {
                        data.illustrators = td
                            .select(&a_sel)
                            .map(|a| clean_text(&a.text().collect::<String>()))
                            .collect();
                    }
                    "Genre" | "ジャンル" => {
                        data.genres = td
                            .select(&a_sel)
                            .map(|a| clean_text(&a.text().collect::<String>()))
                            .collect();
                    }
                    _ => {}
                }
            }
        }
    }

    // Description
    if let Ok(sel) = Selector::parse("div.work_parts_container") {
        if let Some(el) = document.select(&sel).next() {
            data.description = Some(extract_text_with_breaks(el));
        }
    }

    // Tags
    if let Ok(sel) = Selector::parse("div.main_genre a") {
        data.tags = document
            .select(&sel)
            .map(|el| clean_text(&el.text().collect::<String>()))
            .filter(|s| !s.is_empty())
            .collect();
    }

    // Cover image
    let cover_selectors = [
        "div.product-slider-data div[data-src]",
        "picture.product_img source[srcset]",
        "img.target_type",
    ];

    for selector_str in cover_selectors {
        if data.cover_image.is_some() {
            break;
        }
        if let Ok(sel) = Selector::parse(selector_str) {
            if let Some(el) = document.select(&sel).next() {
                let src = el
                    .value()
                    .attr("data-src")
                    .or_else(|| el.value().attr("src"))
                    .or_else(|| el.value().attr("srcset"));
                if let Some(s) = src {
                    data.cover_image = Some(normalize_url(s));
                }
            }
        }
    }

    // Screenshots
    if let Ok(sel) = Selector::parse("div.product-slider-data div[data-src]") {
        data.screenshots = document
            .select(&sel)
            .filter_map(|el| el.value().attr("data-src"))
            .map(normalize_url)
            .collect();
    }

    // Final geo-blocking check
    if !data.geo_blocked {
        let has_essential = data.cover_image.is_some()
            || data.description.is_some()
            || !data.screenshots.is_empty()
            || !data.genres.is_empty()
            || data.circle.is_some();

        if !has_essential {
            data.geo_blocked = true;
        }
    }

    Some(data)
}

/// Parse DLSite search results HTML
pub fn parse_search_html(html: &str) -> Vec<SearchResult> {
    use crate::detect::detect_dlsite_code;

    let document = Html::parse_document(html);
    let mut results = Vec::new();

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
                if let Some(c) = detect_dlsite_code(href) {
                    code = c;
                }
            }
        }

        if let Some(maker_link) = item.select(&maker_selector).next() {
            creator = Some(maker_link.text().collect::<String>().trim().to_string());
        }

        if let Ok(img_sel) = Selector::parse("img") {
            if let Some(img) = item.select(&img_sel).next() {
                if let Some(src) = img
                    .value()
                    .attr("src")
                    .or_else(|| img.value().attr("data-src"))
                {
                    thumbnail = Some(normalize_url(src));
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

/// Build plugin-compatible JSON from raw data
pub fn build_plugin_json(
    product_id: &str,
    api_json: Option<&serde_json::Value>,
    scraped: Option<&ScrapedData>,
) -> serde_json::Value {
    // Extract from API JSON first
    let (mut title, mut circle, short_desc, price, mut release_date, mut tags) =
        if let Some(data) = api_json {
            let date_raw = data["regist_date"].as_str().unwrap_or("");
            let date_clean = if date_raw.is_empty() {
                None
            } else {
                Some(
                    date_raw
                        .split_whitespace()
                        .next()
                        .unwrap_or(date_raw)
                        .to_string(),
                )
            };

            (
                data["work_name"].as_str().map(|s| s.to_string()),
                data["maker_name"].as_str().map(|s| s.to_string()),
                data["intro_s"].as_str().unwrap_or(""),
                data["price"].as_u64().unwrap_or(0),
                date_clean,
                data["genres"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v["name"].as_str().map(|s| s.to_string()))
                            .collect::<Vec<String>>()
                    })
                    .unwrap_or_default(),
            )
        } else {
            (None, None, "", 0, None, Vec::new())
        };

    // Override with scraped data
    if let Some(s) = scraped {
        if s.title.is_some() {
            title = s.title.clone();
        }
        if s.circle.is_some() {
            circle = s.circle.clone();
        }
        if let Some(d) = &s.release_date {
            release_date = Some(d.split_whitespace().next().unwrap_or(d).to_string());
        }
        if !s.tags.is_empty() {
            tags = s.tags.clone();
        }
    }

    let description = scraped
        .and_then(|s| s.description.clone())
        .unwrap_or_else(|| short_desc.to_string());

    let screenshots: Vec<serde_json::Value> = scraped
        .map(|s| {
            s.screenshots
                .iter()
                .map(|url| serde_json::json!({ "FilePath": url }))
                .collect()
        })
        .unwrap_or_default();

    serde_json::json!({
        "product_id": product_id,
        "source": "dlsite",
        "title": title,
        "circle": circle,
        "creator": circle,
        "description": description,
        "release_date": release_date,
        "tags": tags,
        "screenshots": screenshots,
        "voice_actors": scraped.map(|s| s.voice_actors.clone()).unwrap_or_default(),
        "authors": scraped.map(|s| s.authors.clone()).unwrap_or_default(),
        "illustrators": scraped.map(|s| s.illustrators.clone()).unwrap_or_default(),
        "scenarios": scraped.map(|s| s.scenarios.clone()).unwrap_or_default(),
        "musicians": scraped.map(|s| s.musicians.clone()).unwrap_or_default(),
        "writers": scraped.map(|s| s.writers.clone()).unwrap_or_default(),
        "brand": scraped.and_then(|s| s.brand.clone()),
        "publisher": scraped.and_then(|s| s.publisher.clone()),
        "series": scraped.and_then(|s| s.series.clone()),
        "page_count": scraped.and_then(|s| s.page_count),
        "file_size": scraped.and_then(|s| s.file_size.clone()),
        "update_date": scraped.and_then(|s| s.update_date.clone()),
        "genres": scraped.map(|s| s.genres.clone()).unwrap_or_default(),
        "sample_images": scraped.map(|s| s.screenshots.clone()).unwrap_or_default(),
        "cover_image": scraped.and_then(|s| s.cover_image.clone()),
        "geo_blocked": scraped.map(|s| s.geo_blocked).unwrap_or(false),
        "dlsite": {
            "id": product_id,
            "code": product_id,
            "price": price.to_string(),
            "url": format!("https://www.dlsite.com/pro/work/=/product_id/{}.html", product_id)
        },
        "common": {
            "dlsite_id": product_id
        }
    })
}

/// Build plugin JSON as string
pub fn build_plugin_json_string(
    product_id: &str,
    api_json: Option<&serde_json::Value>,
    scraped: Option<&ScrapedData>,
) -> String {
    build_plugin_json(product_id, api_json, scraped).to_string()
}

// === Helper functions ===

fn merge_scraped_data(meta: &mut ProductMetadata, scraped: &ScrapedData) {
    meta.geo_blocked = scraped.geo_blocked;

    if meta.title.is_none() {
        meta.title = scraped.title.clone();
    }
    if meta.creator.is_none() {
        meta.creator = scraped.circle.clone();
    }
    if meta.description.is_none() {
        meta.description = scraped.description.clone();
    }
    if meta.tags.is_empty() {
        meta.tags = scraped.tags.clone();
    }
    if meta.file_size.is_none() {
        meta.file_size = scraped.file_size.clone();
    }

    // Store extras
    meta.extras = serde_json::json!({
        "voice_actors": scraped.voice_actors,
        "authors": scraped.authors,
        "illustrators": scraped.illustrators,
        "scenarios": scraped.scenarios,
        "musicians": scraped.musicians,
        "writers": scraped.writers,
        "brand": scraped.brand,
        "publisher": scraped.publisher,
        "series": scraped.series,
        "page_count": scraped.page_count,
        "update_date": scraped.update_date,
        "cover_image": scraped.cover_image,
        "sample_images": scraped.screenshots,
        "screenshots": scraped.screenshots
    });
}

fn clean_text(text: &str) -> String {
    text.replace('\n', " ")
        .replace('\r', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn extract_text_with_breaks(element: scraper::ElementRef) -> String {
    let mut text = String::new();
    for node in element.descendants() {
        if let Some(el) = node.value().as_element() {
            if el.name() == "br" {
                text.push('\n');
            }
        } else if let Some(t) = node.value().as_text() {
            let s = t.trim();
            if !s.is_empty() {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push(' ');
                }
                text.push_str(s);
            }
        }
    }
    text
}

fn normalize_url(src: &str) -> String {
    if src.starts_with("//") {
        format!("https:{}", src)
    } else if src.starts_with("http") {
        src.to_string()
    } else {
        format!("https://www.dlsite.com{}", src)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_api_json() {
        let json = r#"[{"work_name": "Test Title", "maker_name": "Test Circle", "price": 1000}]"#;
        let meta = parse_api_json("RJ123456", json).unwrap();

        assert_eq!(meta.title, Some("Test Title".to_string()));
        assert_eq!(meta.creator, Some("Test Circle".to_string()));
        assert_eq!(meta.price, Some(1000));
    }

    #[test]
    fn test_parse_html_basic() {
        let html = r##"
        <html>
        <body>
            <h1 id="work_name">Test Title</h1>
            <div id="work_right">
                <span class="maker_name"><a href="#">Test Circle</a></span>
            </div>
            <div class="work_parts_container">Test Description</div>
        </body>
        </html>
        "##;

        let data = parse_html(html).unwrap();
        assert_eq!(data.title, Some("Test Title".to_string()));
        assert_eq!(data.circle, Some("Test Circle".to_string()));
        assert_eq!(data.description, Some("Test Description".to_string()));
        assert!(!data.geo_blocked);
    }

    #[test]
    fn test_parse_dlsite_combined() {
        let json = r#"[{"work_name": "API Title", "maker_name": "API Circle"}]"#;
        let html = r##"
        <html><body>
            <h1 id="work_name">HTML Title</h1>
            <div id="work_right"><span class="maker_name"><a href="#">HTML Circle</a></span></div>
        </body></html>
        "##;

        let meta = parse_dlsite("RJ123456", Some(json), Some(html)).unwrap();

        // API data takes precedence for title/creator
        assert_eq!(meta.title, Some("API Title".to_string()));
        assert_eq!(meta.creator, Some("API Circle".to_string()));
    }
}
