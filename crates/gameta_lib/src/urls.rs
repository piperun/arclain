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

    /// Convert a DLSite cover image URL to a thumbnail URL (240x240).
    /// This is more reliable than constructing URLs from scratch since
    /// the folder path comes directly from DLSite.
    ///
    /// Examples:
    /// - `modpub/.../RJ360420_img_main.jpg` -> `resize/.../RJ360420_img_main_240x240.jpg`
    /// - `resize/.../RJ360420_img_main.jpg` -> `resize/.../RJ360420_img_main_240x240.jpg`
    pub fn cover_to_thumbnail(cover_url: &str) -> Option<String> {
        // Must be a DLSite image URL
        if !cover_url.contains("img.dlsite.jp") {
            return None;
        }

        let mut url = cover_url.to_string();

        // Convert modpub to resize if needed
        if url.contains("/modpub/") {
            url = url.replace("/modpub/", "/resize/");
        }

        // Add _240x240 before the file extension if not already present
        if !url.contains("_240x240") {
            // Find the last occurrence of _img_main or _img_sam before .jpg/.png
            if let Some(ext_pos) = url.rfind(".jpg").or_else(|| url.rfind(".png")) {
                // Insert _240x240 before the extension
                url.insert_str(ext_pos, "_240x240");
            }
        }

        Some(url)
    }

    /// Construct a CDN thumbnail URL (240x240) for a product.
    /// Returns None if the product ID format is unrecognized.
    ///
    /// Prefer `cover_to_thumbnail()` when you have the cover URL from scraping,
    /// as it's more reliable (uses DLSite's actual folder structure).
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
        // e.g., 360420 -> 361000, 1005126 -> 1006000
        let folder_num = ((numeric / 1000) + 1) * 1000;
        // Folder digit count matches product ID digit count
        // RJ360420 -> RJ361000 (6 digits), VJ01005126 -> VJ01006000 (8 digits)
        let digit_count = numeric_str.len();
        let folder = format!("{}{:0width$}", prefix, folder_num, width = digit_count);

        // Use resize path with 240x240 thumbnails
        Some(format!(
            "https://img.dlsite.jp/resize/images2/work/{}/{}/{}_img_main_240x240.jpg",
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

        #[test]
        fn test_cover_to_thumbnail() {
            // modpub -> resize with _240x240
            assert_eq!(
                cover_to_thumbnail("https://img.dlsite.jp/modpub/images2/work/doujin/RJ361000/RJ360420_img_main.jpg"),
                Some("https://img.dlsite.jp/resize/images2/work/doujin/RJ361000/RJ360420_img_main_240x240.jpg".to_string())
            );

            // Already resize, just add _240x240
            assert_eq!(
                cover_to_thumbnail("https://img.dlsite.jp/resize/images2/work/professional/VJ01006000/VJ01005126_img_main.jpg"),
                Some("https://img.dlsite.jp/resize/images2/work/professional/VJ01006000/VJ01005126_img_main_240x240.jpg".to_string())
            );

            // Already has _240x240 - unchanged
            assert_eq!(
                cover_to_thumbnail("https://img.dlsite.jp/resize/images2/work/doujin/RJ361000/RJ360420_img_main_240x240.jpg"),
                Some("https://img.dlsite.jp/resize/images2/work/doujin/RJ361000/RJ360420_img_main_240x240.jpg".to_string())
            );

            // Non-DLSite URL returns None
            assert_eq!(cover_to_thumbnail("https://example.com/image.jpg"), None);
        }
    }
}
