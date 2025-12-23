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

    tracing::info!(
        "[DLsite API] Parsed: title={}, creator={}, price={:?}, rating={:?}, genres={}",
        meta.title.is_some(),
        meta.creator.is_some(),
        meta.price,
        meta.rating,
        meta.genres.len()
    );

    Ok(meta)
}

/// Scraped data from HTML
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
    /// Whether the page was geo-blocked
    pub geo_blocked: bool,
}

/// Helper to clean text
fn clean_text(text: &str) -> String {
    text.replace('\n', " ")
        .replace('\r', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse DLSite HTML page for additional data
pub fn parse_html_response(html: &str) -> Option<ScrapedData> {
    let document = Html::parse_document(html);

    let mut data = ScrapedData {
        title: None,
        circle: None,
        release_date: None,
        update_date: None,
        tags: Vec::new(),
        description: None,
        cover_image: None,
        screenshots: Vec::new(),
        voice_actors: Vec::new(),
        authors: Vec::new(),
        illustrators: Vec::new(),
        scenarios: Vec::new(),
        musicians: Vec::new(),
        writers: Vec::new(),
        brand: None,
        publisher: None,
        series: None,
        page_count: None,
        file_size: None,
        genres: Vec::new(),
        geo_blocked: false,
    };

    // Check for geo-blocked page (common issue for non-JP users)
    let html_lower = html.to_lowercase();
    if html_lower.contains("お住いの国・地域からは本作品は購入できません")
        || html_lower.contains("this product cannot be purchased")
    {
        tracing::warn!("[DLsite HTML] Page is geo-blocked - limited content available");
        data.geo_blocked = true;
    }

    // Title
    if let Ok(sel) = Selector::parse("h1#work_name") {
        if let Some(el) = document.select(&sel).next() {
            data.title = Some(clean_text(&el.text().collect::<String>()));
        }
    }
    // Fallback title selector
    if data.title.is_none() {
        if let Ok(sel) = Selector::parse("h1.topicpath_last") {
            if let Some(el) = document.select(&sel).next() {
                data.title = Some(clean_text(&el.text().collect::<String>()));
            }
        }
    }

    // Circle/maker from header (backup)
    if let Ok(sel) = Selector::parse("span.maker_name a") {
        if let Some(el) = document.select(&sel).next() {
            data.circle = Some(clean_text(&el.text().collect::<String>()));
        }
    }

    // Work Maker Table (Brand, Publisher, etc.)
    if let Ok(table_sel) = Selector::parse("table#work_maker tr") {
        for row in document.select(&table_sel) {
            let th_sel = Selector::parse("th").unwrap();
            let td_sel = Selector::parse("td").unwrap();

            if let (Some(th), Some(td)) = (row.select(&th_sel).next(), row.select(&td_sel).next()) {
                let header = clean_text(&th.text().collect::<String>());
                let content = clean_text(&td.text().collect::<String>());

                match header.as_str() {
                    "Brand" | "ブランド" => data.brand = Some(content),
                    "Circle" | "サークル" => data.circle = Some(content), // Override header circle
                    "Publisher" | "出版社" => data.publisher = Some(content),
                    "Label" | "レーベル" => {
                        // Sometimes label is brand, sometimes separate. Use brand field for now or add label?
                        // Python scraper puts it into brand/maker properties.
                        if data.brand.is_none() {
                            data.brand = Some(content);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // Work Outline Table (The big metadata table)
    if let Ok(table_sel) = Selector::parse("table#work_outline tr") {
        for row in document.select(&table_sel) {
            let th_sel = Selector::parse("th").unwrap();
            let td_sel = Selector::parse("td").unwrap();
            let a_sel = Selector::parse("a").unwrap();

            if let (Some(th), Some(td)) = (row.select(&th_sel).next(), row.select(&td_sel).next()) {
                let header = clean_text(&th.text().collect::<String>());
                // For extraction involving links, we might need inner text of links

                match header.as_str() {
                    "Release Date" | "販売日" | "Regist Date" => {
                        data.release_date = Some(clean_text(&td.text().collect::<String>()));
                    }
                    "Update Date" | "更新日" => {
                        data.update_date = Some(clean_text(&td.text().collect::<String>()));
                    }
                    "Series" | "シリーズ名" => {
                        data.series = Some(clean_text(&td.text().collect::<String>()));
                    }
                    "Page Count" | "ページ数" | "Total Pages" => {
                        let text = clean_text(&td.text().collect::<String>());
                        // Extract number from "24 pages" etc.
                        if let Some(num_str) = text.split_whitespace().next() {
                            if let Ok(num) = num_str.parse::<i64>() {
                                data.page_count = Some(num);
                            }
                        }
                    }
                    "File size" | "ファイル容量" | "Total size" => {
                        data.file_size = Some(clean_text(&td.text().collect::<String>()));
                    }
                    "Voice Actor" | "声優" => {
                        data.voice_actors = td
                            .select(&a_sel)
                            .map(|a| clean_text(&a.text().collect::<String>()))
                            .collect();
                        if data.voice_actors.is_empty() {
                            // Try text content if no links
                            data.voice_actors
                                .push(clean_text(&td.text().collect::<String>()));
                        }
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
                    "Scenario" | "シナリオ" => {
                        data.scenarios = td
                            .select(&a_sel)
                            .map(|a| clean_text(&a.text().collect::<String>()))
                            .collect();
                    }
                    "Music" | "音楽" => {
                        data.musicians = td
                            .select(&a_sel)
                            .map(|a| clean_text(&a.text().collect::<String>()))
                            .collect();
                    }
                    "Writer" | "ライター" => {
                        data.writers = td
                            .select(&a_sel)
                            .map(|a| clean_text(&a.text().collect::<String>()))
                            .collect();
                    }
                    "Genre" | "ジャンル" => {
                        // Sometimes duplicate of tags, but useful
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
            data.description = Some(clean_text(&el.text().collect::<String>()));
        }
    }
    // Fallback description from meta tag
    if data.description.is_none() {
        if let Ok(sel) = Selector::parse("meta[name=\"description\"]") {
            if let Some(el) = document.select(&sel).next() {
                if let Some(content) = el.value().attr("content") {
                    data.description = Some(clean_text(content));
                }
            }
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

    // Cover image - try multiple selectors
    let cover_selectors = [
        "div.product-slider-data div[data-src]", // Newer structure
        "div.product_slider_data div[data-src]", // Alternative naming
        "picture.product_img source[srcset]",    // Picture element
        "img.target_type",                       // Direct img tag
        "div#work_left img.work_thumbnail",      // Thumbnail fallback
    ];

    for selector_str in cover_selectors {
        if data.cover_image.is_some() {
            break;
        }
        if let Ok(sel) = Selector::parse(selector_str) {
            if let Some(el) = document.select(&sel).next() {
                // Try data-src first, then src, then srcset
                let src = el
                    .value()
                    .attr("data-src")
                    .or_else(|| el.value().attr("src"))
                    .or_else(|| el.value().attr("srcset"));
                if let Some(s) = src {
                    let url = if s.starts_with("//") {
                        format!("https:{}", s)
                    } else if s.starts_with("http") {
                        s.to_string()
                    } else {
                        format!("https://www.dlsite.com{}", s)
                    };
                    data.cover_image = Some(url);
                    tracing::debug!("[DLsite HTML] Cover found with selector: {}", selector_str);
                }
            }
        }
    }

    // Screenshots - try multiple selectors
    let screenshot_selectors = [
        "div.product-slider-data div[data-src]",
        "div.product_slider_data div[data-src]",
        "ul.product_slider li img[data-src]",
    ];

    for selector_str in screenshot_selectors {
        if !data.screenshots.is_empty() {
            break;
        }
        if let Ok(sel) = Selector::parse(selector_str) {
            data.screenshots = document
                .select(&sel)
                .skip(1) // First is usually cover
                .filter_map(|el| {
                    el.value()
                        .attr("data-src")
                        .or_else(|| el.value().attr("src"))
                })
                .map(|src| {
                    if src.starts_with("//") {
                        format!("https:{}", src)
                    } else if src.starts_with("http") {
                        src.to_string()
                    } else {
                        format!("https://www.dlsite.com{}", src)
                    }
                })
                .collect();

            if !data.screenshots.is_empty() {
                tracing::debug!(
                    "[DLsite HTML] Screenshots found with selector: {}",
                    selector_str
                );
            }
        }
    }

    // Log what we found
    tracing::info!(
        "[DLsite HTML] Parsed: title={}, circle={}, cover={}, screenshots={}, voice_actors={}, genres={}",
        data.title.is_some(),
        data.circle.is_some(),
        data.cover_image.is_some(),
        data.screenshots.len(),
        data.voice_actors.len(),
        data.genres.len()
    );

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
