//! DLSite API URL construction and connectivity checks.

/// Connectivity check result
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectivityResult {
    /// Successfully connected, contains detected region
    Ok { region: String },
    /// Geo-blocked
    GeoBlocked,
    /// Network or other error
    Error(String),
}

/// Get the DLSite site_id based on product ID prefix.
///
/// Different product prefixes map to different DLSite storefronts:
/// - VJ: pro (Visual Novels / Galge)
/// - RJ: maniax (Doujin R18)
/// - BJ: books (Books/Comics)
/// - RE: maniax (English R18)
pub fn get_site_id(product_id: &str) -> &'static str {
    let prefix: String = product_id.chars().take(2).collect();
    match prefix.as_str() {
        "VJ" => "pro",
        "RJ" => "maniax",
        "BJ" => "books",
        "RE" => "maniax",
        _ => "home",
    }
}

/// Construct the ajax API URL for a product.
pub fn ajax_url(product_id: &str) -> String {
    let site_id = get_site_id(product_id);
    format!(
        "https://www.dlsite.com/{}/product/info/ajax?product_id={}",
        site_id, product_id
    )
}

/// Construct the HTML work page URL for a product.
pub fn html_url(product_id: &str) -> String {
    let site_id = get_site_id(product_id);
    format!(
        "https://www.dlsite.com/{}/work/=/product_id/{}.html",
        site_id, product_id
    )
}

/// Geo-block detection patterns found in blocked pages
pub const GEO_BLOCK_PATTERNS: &[&str] = &[
    "お住いの国・地域からは本作品は購入できません",
    "this product cannot be purchased",
    "このページはお住まいの地域からは表示できません",
    "this page cannot be displayed",
    "access denied",
    "region restricted",
    "not available in your country",
    "geographic restrictions",
];

/// Check if HTML content indicates geo-blocking
pub fn is_geo_blocked(html: &str) -> bool {
    let html_lower = html.to_lowercase();
    GEO_BLOCK_PATTERNS
        .iter()
        .any(|pattern| html_lower.contains(&pattern.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_site_id() {
        assert_eq!(get_site_id("VJ012345"), "pro");
        assert_eq!(get_site_id("RJ294126"), "maniax");
        assert_eq!(get_site_id("BJ370220"), "books");
        assert_eq!(get_site_id("RE123456"), "maniax");
        assert_eq!(get_site_id("XX999999"), "home");
    }

    #[test]
    fn test_ajax_url() {
        assert_eq!(
            ajax_url("VJ012345"),
            "https://www.dlsite.com/pro/product/info/ajax?product_id=VJ012345"
        );
    }

    #[test]
    fn test_html_url() {
        assert_eq!(
            html_url("VJ012345"),
            "https://www.dlsite.com/pro/work/=/product_id/VJ012345.html"
        );
    }

    #[test]
    fn test_is_geo_blocked() {
        assert!(is_geo_blocked(
            "お住いの国・地域からは本作品は購入できません"
        ));
        assert!(is_geo_blocked("This product cannot be purchased"));
        assert!(!is_geo_blocked("<html><body>Normal content</body></html>"));
    }
}
