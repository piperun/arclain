use bitflags::bitflags;

bitflags! {
    /// Options for controlling DLSite metadata fetching behavior.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
    pub struct DlsiteFetchOptions: u8 {
        /// Fetch from the JSON API (fast, basic metadata)
        const JSON_API = 1 << 0;

        /// Scrape the full HTML work page (slower, comprehensive metadata)
        const HTML_SCRAPE = 1 << 1;

        /// Convenience set equivalent to JSON_API
        const FAST_ONLY = Self::JSON_API.bits();

        /// Fetch everything available
        const ALL = Self::JSON_API.bits() | Self::HTML_SCRAPE.bits();
    }
}
