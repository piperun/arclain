use bitflags::bitflags;

bitflags! {
    /// Options for controlling DLSite metadata fetching behavior.
    ///
    /// This allows granular control over which sources are queried (API vs HTML)
    /// and ensures type safety when configuring fetch strategies.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
    pub struct DlsiteFetchOptions: u8 {
        /// Fetch from the JSON API (fast, basic metadata)
        /// Currently maps to: https://www.dlsite.com/{site_id}/product/info/ajax?product_id={id}
        const JSON_API = 1 << 0;

        /// Scrape the full HTML work page (slower, comprehensive metadata)
        /// Currently maps to: https://www.dlsite.com/{site_id}/work/=/product_id/{id}.html
        const HTML_SCRAPE = 1 << 1;

        /// Convenience set equivalent to JSON_API
        const FAST_ONLY = Self::JSON_API.bits();

        /// Fetch everything available
        const ALL = Self::JSON_API.bits() | Self::HTML_SCRAPE.bits();
    }
}
