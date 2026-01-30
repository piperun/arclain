//! URL builders for metadata sources
//!
//! These are pure functions that construct URLs - no HTTP fetching.

/// DLSite URL builders
pub mod dlsite {
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

    /// Construct a search URL.
    pub fn search_url(query: &str) -> String {
        format!(
            "https://www.dlsite.com/home/fsr/=/language/jp/keyword/{}/order/trend/per_page/30/page/1",
            urlencoding::encode(query)
        )
    }

    /// Construct a CDN thumbnail URL (240x240) for a product.
    /// Returns None if the product ID format is unrecognized.
    pub fn thumbnail_url(product_id: &str) -> Option<String> {
        // Extract prefix (RJ, VJ, BJ, RE) and numeric part
        if product_id.len() < 3 {
            return None;
        }
        let prefix = &product_id[..2];
        let numeric_str = &product_id[2..];
        let numeric: u64 = numeric_str.parse().ok()?;

        // Determine category for CDN path
        let category = match prefix {
            "VJ" => "professional",
            "RJ" | "RE" => "doujin",
            "BJ" => "books",
            _ => return None,
        };

        // Calculate folder: ceiling of (numeric / 1000) * 1000
        // e.g., 1532102 -> 1533000
        let folder_num = ((numeric / 1000) + 1) * 1000;
        let folder = format!("{}{:08}", prefix, folder_num);

        // Use modpub path which is more accessible than resize
        Some(format!(
            "https://img.dlsite.jp/modpub/images2/work/{}/{}/{}_img_sam.jpg",
            category, folder, product_id
        ))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_get_site_id() {
            assert_eq!(get_site_id("VJ012345"), "pro");
            assert_eq!(get_site_id("RJ294126"), "maniax");
            assert_eq!(get_site_id("BJ370220"), "books");
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
    }
}
