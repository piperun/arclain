//! Pure matching + ranking logic for the unified search palette.
//!
//! Free of egui and signal/Arc machinery so it stays deterministic and
//! unit-testable. The view layer turns these hits into rows and uses
//! [`match_range`] for substring highlighting; the app layer builds the
//! [`TabSummary`] inputs from live `TabState` signals.

use crate::core::tabs::TabId;

/// Upper bound on file rows surfaced for one query. The active archive
/// can hold thousands of entries; rendering every match would stall the
/// palette and bury the (tabs-first) results. Matches past the cap are
/// dropped — narrowing the query is how you reach them.
pub const MAX_FILE_HITS: usize = 50;

/// Render-agnostic snapshot of an open tab, built by the caller from
/// `TabState` signals (`game_metadata`, `archive_path`, `entries`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabSummary {
    pub id: TabId,
    /// Product code — `ProductMetadataSummary::product_id` (empty when
    /// no metadata).
    pub code: String,
    /// Display title — `ProductMetadataSummary::title`, falling back to
    /// the file stem.
    pub title: String,
    /// Maker / circle — `ProductMetadataSummary::creator` (empty when
    /// unknown).
    pub maker: String,
    /// Archive file name (no directory) of the tab's loaded archive.
    pub file: String,
    /// Number of entries in the tab's archive.
    pub entry_count: usize,
    /// True for the currently-active tab (renders an "active" badge).
    pub active: bool,
}

/// One flattened result row. Tabs sort ahead of files; the view inserts
/// a group header at each kind transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchHit {
    Tab(TabSummary),
    File { path: String },
}

/// Build the flattened result list for `query`.
///
/// - Empty query → every open tab (the palette doubles as a tab switcher
///   on focus), no files.
/// - Non-empty query → tabs whose code/title/maker/file contains the
///   query (case-insensitive), then up to [`MAX_FILE_HITS`] active-archive
///   entry paths containing the query. Tabs always precede files.
pub fn build_hits(query: &str, tabs: &[TabSummary], active_paths: &[&str]) -> Vec<SearchHit> {
    let q = query.trim().to_lowercase();
    let mut hits = Vec::new();

    if q.is_empty() {
        hits.extend(tabs.iter().cloned().map(SearchHit::Tab));
        return hits;
    }

    for t in tabs {
        if tab_matches(t, &q) {
            hits.push(SearchHit::Tab(t.clone()));
        }
    }
    for path in active_paths
        .iter()
        .filter(|p| p.to_lowercase().contains(&q))
        .take(MAX_FILE_HITS)
    {
        hits.push(SearchHit::File {
            path: (*path).to_string(),
        });
    }
    hits
}

fn tab_matches(t: &TabSummary, q_lower: &str) -> bool {
    [&t.code, &t.title, &t.maker, &t.file]
        .iter()
        .any(|f| f.to_lowercase().contains(q_lower))
}

/// Byte range within `haystack` of the first case-insensitive occurrence
/// of `needle`, for substring highlighting. `None` if `needle` is empty
/// or absent.
///
/// The range indexes the ORIGINAL `haystack` so the view can slice it
/// directly. Mapping through char counts is exact for the scripts in
/// play here — ASCII product codes, Latin titles, and Japanese maker
/// names (no case) all lowercase 1:1 in char count.
pub fn match_range(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    let hay_lower = haystack.to_lowercase();
    let needle_lower = needle.to_lowercase();
    let lower_byte = hay_lower.find(&needle_lower)?;
    let char_start = hay_lower[..lower_byte].chars().count();
    let char_len = needle_lower.chars().count();
    let start = nth_char_byte(haystack, char_start);
    let end = nth_char_byte(haystack, char_start + char_len);
    Some((start, end))
}

fn nth_char_byte(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map(|(b, _)| b).unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: u64, code: &str, title: &str, maker: &str, file: &str) -> TabSummary {
        TabSummary {
            id: TabId(id),
            code: code.to_string(),
            title: title.to_string(),
            maker: maker.to_string(),
            file: file.to_string(),
            entry_count: 0,
            active: false,
        }
    }

    fn tab_ids(hits: &[SearchHit]) -> Vec<u64> {
        hits.iter()
            .filter_map(|h| match h {
                SearchHit::Tab(t) => Some(t.id.0),
                SearchHit::File { .. } => None,
            })
            .collect()
    }

    fn file_paths(hits: &[SearchHit]) -> Vec<String> {
        hits.iter()
            .filter_map(|h| match h {
                SearchHit::File { path } => Some(path.clone()),
                SearchHit::Tab(_) => None,
            })
            .collect()
    }

    #[test]
    fn empty_query_lists_all_tabs_in_order_and_no_files() {
        let tabs = vec![
            tab(1, "RJ000111", "Alpha", "Aria", "alpha.rar"),
            tab(2, "RJ000222", "Beta", "Coralt", "beta.rar"),
        ];
        let hits = build_hits("", &tabs, &["alpha.rar/cover.png"]);
        assert_eq!(tab_ids(&hits), vec![1, 2]);
        assert!(
            file_paths(&hits).is_empty(),
            "empty query must not list files"
        );
    }

    #[test]
    fn whitespace_only_query_behaves_like_empty() {
        let tabs = vec![tab(1, "RJ1", "Alpha", "Aria", "a.rar")];
        let hits = build_hits("   ", &tabs, &["a/b.txt"]);
        assert_eq!(tab_ids(&hits), vec![1]);
        assert!(file_paths(&hits).is_empty());
    }

    #[test]
    fn tab_matches_on_each_field_case_insensitively() {
        let tabs = vec![tab(
            7,
            "RJ000999",
            "Sample Pack",
            "Studio Glow",
            "[Glow] x.rar",
        )];
        // code
        assert_eq!(tab_ids(&build_hits("rj000999", &tabs, &[])), vec![7]);
        // title
        assert_eq!(tab_ids(&build_hits("PACK", &tabs, &[])), vec![7]);
        // maker
        assert_eq!(tab_ids(&build_hits("glow", &tabs, &[])), vec![7]);
        // file name
        assert_eq!(tab_ids(&build_hits("x.rar", &tabs, &[])), vec![7]);
    }

    #[test]
    fn non_matching_query_returns_nothing() {
        let tabs = vec![tab(1, "RJ1", "Alpha", "Aria", "a.rar")];
        let hits = build_hits("zzzznope", &tabs, &["a/b.txt"]);
        assert!(hits.is_empty());
    }

    #[test]
    fn files_match_scoped_to_active_paths_case_insensitively() {
        let tabs = vec![tab(1, "RJ1", "Alpha", "Aria", "a.rar")];
        let paths = ["RJ1/Scene_01.txt", "RJ1/img_main.JPG", "cover.png"];
        let hits = build_hits("jpg", &tabs, &paths);
        assert_eq!(file_paths(&hits), vec!["RJ1/img_main.JPG".to_string()]);
    }

    #[test]
    fn tabs_always_precede_files() {
        let tabs = vec![tab(1, "scene", "Title", "Maker", "scene.rar")];
        let paths = ["scene_01.txt"];
        let hits = build_hits("scene", &tabs, &paths);
        // First hit is the tab, then the file.
        assert!(matches!(hits[0], SearchHit::Tab(_)));
        assert!(matches!(hits[1], SearchHit::File { .. }));
    }

    #[test]
    fn file_hits_capped_at_max() {
        let tabs: Vec<TabSummary> = vec![];
        let owned: Vec<String> = (0..MAX_FILE_HITS + 25)
            .map(|i| format!("dir/match_{i}.txt"))
            .collect();
        let paths: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
        let hits = build_hits("match", &tabs, &paths);
        assert_eq!(file_paths(&hits).len(), MAX_FILE_HITS);
    }

    #[test]
    fn match_range_basic_ascii() {
        assert_eq!(match_range("scene_01.txt", "scene"), Some((0, 5)));
        assert_eq!(match_range("img_main.jpg", "main"), Some((4, 8)));
    }

    #[test]
    fn match_range_is_case_insensitive_but_indexes_original() {
        // Query "cde" matches "Cde" — range points at the original casing.
        let (s, e) = match_range("ABCdef", "cde").unwrap();
        assert_eq!(&"ABCdef"[s..e], "Cde");
    }

    #[test]
    fn match_range_absent_or_empty_is_none() {
        assert_eq!(match_range("hello", "zzz"), None);
        assert_eq!(match_range("hello", ""), None);
    }

    #[test]
    fn match_range_handles_multibyte_prefix() {
        // 4 Japanese chars (3 bytes each) then "test"; the highlight must
        // land on the ASCII run, not split a multibyte boundary.
        let hay = "サークルtest";
        let (s, e) = match_range(hay, "TEST").unwrap();
        assert_eq!(&hay[s..e], "test");
    }
}
