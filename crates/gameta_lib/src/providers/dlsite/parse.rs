//! DLSite response parsing

use gameta_core::{MetadataSource, ParseError, ProductMetadata, SearchResult};
use scraper::{Html, Selector};
use std::collections::HashSet;

// Re-use ScrapedData from parsers module (single source of truth)
pub use crate::parsers::dlsite::ScrapedData;

/// Parse DLSite API JSON response
pub fn parse_api_response(
    external_id: &str,
    json_str: &str,
) -> Result<ProductMetadata, ParseError> {
    let json: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| ParseError::InvalidFormat(format!("Invalid JSON: {}", e)))?;

    let data = if let Some(arr) = json.as_array() {
        arr.first().cloned().unwrap_or(json.clone())
    } else {
        json
    };

    let mut meta = ProductMetadata::new(MetadataSource::DLSite, external_id);

    meta.title = data["work_name"].as_str().map(|s| s.to_string());
    meta.creator = data["maker_name"].as_str().map(|s| s.to_string());

    meta.release_date = data["regist_date"]
        .as_str()
        .map(|s| s.split_whitespace().next().unwrap_or(s).to_string());

    meta.price = data["price"]
        .as_i64()
        .or_else(|| data["price"].as_str().and_then(|s| s.parse().ok()));
    meta.currency = Some("JPY".to_string());

    meta.rating = data["rate_average_2dp"].as_f64();
    meta.rating_count = data["rate_count"].as_i64();
    meta.purchase_count = data["dl_count"].as_i64();
    meta.favorite_count = data["wishlist_count"].as_i64();
    meta.review_count = data["review_count"].as_i64();

    meta.file_size = data["file_size"].as_str().map(|s| s.to_string());
    meta.file_format = data["file_type"].as_str().map(|s| s.to_string());
    meta.age_rating = data["age_category_string"].as_str().map(|s| s.to_string());

    if let Some(genres) = data["genre"].as_array() {
        meta.genres = genres
            .iter()
            .filter_map(|g| g["name"].as_str().map(|s| s.to_string()))
            .collect();
    }

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

    // Check for geo-blocked page
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

    // Work Maker Table
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
                    let url = if s.starts_with("//") {
                        format!("https:{}", s)
                    } else if s.starts_with("http") {
                        s.to_string()
                    } else {
                        format!("https://www.dlsite.com{}", s)
                    };
                    data.cover_image = Some(url);
                }
            }
        }
    }

    // Screenshots
    if let Ok(sel) = Selector::parse("div.product-slider-data div[data-src]") {
        let potential: Vec<String> = document
            .select(&sel)
            .filter_map(|el| el.value().attr("data-src"))
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

        let mut seen = HashSet::new();
        data.screenshots = potential
            .into_iter()
            .filter(|url| {
                if !seen.insert(url.clone()) {
                    return false;
                }
                if let Some(cover) = &data.cover_image {
                    if url == cover {
                        return false;
                    }
                }
                true
            })
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

/// Parse DLSite search results page
pub fn parse_search_response(html: &str) -> Vec<SearchResult> {
    use super::detect_dlsite_code;

    let document = Html::parse_document(html);
    let mut results = Vec::new();

    let item_selector = Selector::parse("li.search_result_img_box_inner, tr.n_worklist_item")
        .unwrap_or_else(|_| Selector::parse("div").unwrap());
    // Note: work_name is dd not dt in current DLSite HTML
    let title_selector = Selector::parse("dd.work_name a, dt.work_name a, a.work_name")
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
