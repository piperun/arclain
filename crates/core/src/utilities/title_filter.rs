use arclain_db::DieselPool;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

// --- Constants (Factory Defaults) ---

const DEFAULT_INVALID_CHARS: &str = r#"/:*?"<>|\"#;
const DEFAULT_REPLACEMENT: &str = "_";
const DEFAULT_MAX_LENGTH: usize = 255;
const DEFAULT_TRIM_WHITESPACE: bool = true;

// System replacements (hardcoded defaults)
static DEFAULT_SYSTEM_REPLACEMENTS: Lazy<HashMap<String, String>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert("【".to_string(), "[".to_string());
    m.insert("】".to_string(), "]".to_string());
    m.insert("～".to_string(), "~".to_string());
    m.insert("（".to_string(), "(".to_string());
    m.insert("）".to_string(), ")".to_string());
    m
});

// --- Configuration Models ---

/// Configuration for title filtering
#[derive(Debug, Clone, Deserialize)]
pub struct TitleFilterConfig {
    #[serde(default)]
    pub filters: FilterSettings,
    #[serde(default)]
    pub replacements: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FilterSettings {
    #[serde(default = "default_invalid_chars")]
    pub invalid_chars: String,
    #[serde(default = "default_replacement")]
    pub replacement: String,
    #[serde(default = "default_max_length")]
    pub max_length: usize,
    #[serde(default = "default_true")]
    pub trim_whitespace: bool,
}

impl Default for FilterSettings {
    fn default() -> Self {
        Self {
            invalid_chars: DEFAULT_INVALID_CHARS.to_string(),
            replacement: DEFAULT_REPLACEMENT.to_string(),
            max_length: DEFAULT_MAX_LENGTH,
            trim_whitespace: DEFAULT_TRIM_WHITESPACE,
        }
    }
}

// Helper functions for Serde defaults
fn default_invalid_chars() -> String {
    DEFAULT_INVALID_CHARS.to_string()
}
fn default_replacement() -> String {
    DEFAULT_REPLACEMENT.to_string()
}
fn default_max_length() -> usize {
    DEFAULT_MAX_LENGTH
}
fn default_true() -> bool {
    true
}

// --- Service & Caching ---

// Global cache for title filter settings
static FILTER_CACHE: Lazy<Arc<RwLock<TitleFilterConfig>>> = Lazy::new(|| {
    Arc::new(RwLock::new(TitleFilterConfig {
        filters: FilterSettings::default(),
        replacements: DEFAULT_SYSTEM_REPLACEMENTS.clone(),
    }))
});

/// Initialize the title filter service
/// This should be called at application startup
pub fn init(pool: &DieselPool) -> anyhow::Result<()> {
    // 2. Seed system replacements if missing
    seed_system_replacements(pool)?;

    // 3. Load current config from DB into cache
    refresh_cache(pool)?;

    Ok(())
}

/// Refresh the in-memory cache from the database
pub fn refresh_cache(pool: &DieselPool) -> anyhow::Result<()> {
    pool.with_conn(|conn| {
        // Load scalar settings
        let db_settings = arclain_db::get_title_filter_settings(conn)?;

        let filters = FilterSettings {
            invalid_chars: db_settings
                .invalid_chars
                .unwrap_or_else(|| DEFAULT_INVALID_CHARS.to_string()),
            replacement: db_settings
                .replacement
                .unwrap_or_else(|| DEFAULT_REPLACEMENT.to_string()),
            max_length: db_settings.max_length.unwrap_or(DEFAULT_MAX_LENGTH),
            trim_whitespace: db_settings
                .trim_whitespace
                .unwrap_or(DEFAULT_TRIM_WHITESPACE),
        };

        // Load replacements
        let db_replacements = arclain_db::list_title_replacements(conn)?;
        let mut replacements = HashMap::new();

        // Start with system defaults (in case DB is empty/corrupt, though DB should win)
        // Actually, DB list_title_replacements returns ALL replacements, including system ones.
        // So we just use what's in the DB.
        for r in db_replacements {
            replacements.insert(r.original, r.replacement);
        }

        // If DB returned no replacements, fallback to hardcoded defaults (safety net)
        if replacements.is_empty() {
            replacements = DEFAULT_SYSTEM_REPLACEMENTS.clone();
        }

        // Update cache. parking_lot::RwLock has no poisoning, so a panic
        // somewhere upstream while holding the guard does not break
        // subsequent sanitize_title calls (audit finding M3).
        let mut cache = FILTER_CACHE.write();
        *cache = TitleFilterConfig {
            filters,
            replacements,
        };

        tracing::info!("Title filter cache refreshed");
        Ok(())
    })
}

/// Ensure system replacements exist in the DB
fn seed_system_replacements(pool: &DieselPool) -> anyhow::Result<()> {
    pool.with_conn(|conn| {
        // save_title_replacement upserts on `original`, so re-running
        // this on startup overwrites any user customisation that has the
        // same source string. We accept that: system defaults are
        // intentionally sticky for the same key.
        for (original, replacement) in DEFAULT_SYSTEM_REPLACEMENTS.iter() {
            arclain_db::save_title_replacement(conn, original, replacement, true)?;
        }
        Ok(())
    })
}

// --- Public API ---

/// Sanitize a title for use in folder names
///
/// This function replaces invalid filesystem characters with safe alternatives.
/// It uses the cached configuration.
pub fn sanitize_title(title: &str) -> String {
    // parking_lot::RwLock — no poisoning, no .unwrap() needed.
    let cache = FILTER_CACHE.read();
    sanitize_title_with_config(title, &cache)
}

/// Windows device names, which name a device rather than a file
/// whatever directory they appear in and whatever extension follows.
const RESERVED_DEVICE_NAMES: [&str; 24] = [
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
    "COM8", "COM9", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// `candidate` as a single, plain file-name component, or `None` when it
/// cannot be one.
///
/// **This is the security check [`sanitize_title`] is not.** That
/// function is a presentation filter whose character set
/// (`invalid_chars`) is *user configuration* read from the config
/// database: a user who narrows it stops it stripping path separators,
/// and the sanitized string is then joined onto a destination
/// directory. Anything deriving a file name from text this process does
/// not control -- a plugin-reported title, a code detected inside a file
/// name -- must therefore prove the result still names a file *in* that
/// directory rather than steering out of it.
///
/// Rejects, in every case regardless of configuration:
///
/// * nothing to name a file with (empty, or whitespace only);
/// * either path separator, so the result cannot address another
///   directory -- `\` is rejected on Unix too, because a name derived
///   here routinely travels to Windows inside an archive;
/// * `:`, which prefixes a drive (`C:...`) and, on NTFS, opens an
///   alternate data stream (`name:stream`);
/// * `.` and `..`, which name directories rather than files;
/// * control characters, including the NUL that truncates a path at
///   every C API boundary;
/// * a trailing `.` or space, which Windows silently strips -- the file
///   created would not be the name that was checked;
/// * a reserved device name, with or without an extension.
///
/// Returns the trimmed candidate on success, so a caller need not trim
/// separately and cannot re-introduce a rejected trailing space.
pub fn plain_file_component(candidate: &str) -> Option<&str> {
    let candidate = candidate.trim();
    if candidate.is_empty() || candidate == "." || candidate == ".." {
        return None;
    }
    if candidate
        .chars()
        .any(|c| matches!(c, '/' | '\\' | ':') || c.is_control())
    {
        return None;
    }
    // `trim` already removed trailing whitespace; a trailing dot is what
    // is left to reject.
    if candidate.ends_with('.') {
        return None;
    }
    let stem = candidate.split('.').next().unwrap_or(candidate);
    if RESERVED_DEVICE_NAMES
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
    {
        return None;
    }
    Some(candidate)
}

/// Sanitize a title with an explicit configuration (internal logic)
fn sanitize_title_with_config(title: &str, cfg: &TitleFilterConfig) -> String {
    let mut result = title.to_string();

    // Apply custom character replacements first
    for (from, to) in &cfg.replacements {
        result = result.replace(from, to);
    }

    // Replace invalid characters
    let invalid_chars: Vec<char> = cfg.filters.invalid_chars.chars().collect();
    result = result
        .chars()
        .map(|c| {
            if invalid_chars.contains(&c) || c.is_control() {
                cfg.filters.replacement.chars().next().unwrap_or('_')
            } else {
                c
            }
        })
        .collect();

    // Trim whitespace if configured
    if cfg.filters.trim_whitespace {
        result = result.trim().to_string();
    }

    // Truncate if needed
    if result.len() > cfg.filters.max_length {
        result.truncate(cfg.filters.max_length);
        // Ensure we don't cut in the middle of a multi-byte character
        while !result.is_char_boundary(result.len()) && result.len() > 0 {
            result.pop();
        }
    }

    // Ensure we don't return an empty string
    if result.is_empty() {
        result = "untitled".to_string();
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── plain_file_component ────────────────────────────────────────

    #[test]
    fn an_ordinary_name_is_a_plain_component() {
        assert_eq!(
            plain_file_component("Placeholder Title"),
            Some("Placeholder Title")
        );
        assert_eq!(plain_file_component("  padded  "), Some("padded"));
        assert_eq!(plain_file_component("RJ123456"), Some("RJ123456"));
        assert_eq!(
            plain_file_component("v1.2 [patched]"),
            Some("v1.2 [patched]")
        );
        // Not a traversal: no separator, and not `.` or `..` itself.
        assert_eq!(
            plain_file_component("..leading dots"),
            Some("..leading dots")
        );
        assert_eq!(plain_file_component("テストゲーム"), Some("テストゲーム"));
    }

    /// The whole point: these must be refused whatever `invalid_chars`
    /// happens to be configured as, because they steer the file
    /// somewhere other than the directory it is joined onto.
    #[test]
    fn anything_that_could_steer_a_path_is_refused() {
        for hostile in [
            "",
            "   ",
            ".",
            "..",
            "../evil",
            "..\\evil",
            "..\\..\\evil",
            "sub/dir",
            "sub\\dir",
            "name/../..",
            "C:\\x",
            "C:x",
            "name:stream",
            "\\\\server\\share",
            "trailing.",
            "nul\u{0}byte",
            "bell\u{7}",
            "CON",
            "con",
            "nul.txt",
            "LPT9.zip",
        ] {
            assert_eq!(
                plain_file_component(hostile),
                None,
                "{hostile:?} must not be usable as a file name"
            );
        }
    }

    /// A trailing space is refused by being trimmed away rather than by
    /// failing, since the trimmed name is still a perfectly good one.
    #[test]
    fn a_trailing_space_is_trimmed_not_rejected() {
        assert_eq!(plain_file_component("name "), Some("name"));
    }

    /// `sanitize_title` cannot be trusted to do this itself: its
    /// character set is configuration, so a narrowed one passes
    /// separators straight through.
    #[test]
    fn a_narrowed_invalid_char_set_leaves_separators_in_a_sanitized_title() {
        let config = TitleFilterConfig {
            filters: FilterSettings {
                invalid_chars: "*?".to_string(),
                ..FilterSettings::default()
            },
            replacements: HashMap::new(),
        };
        let sanitized = sanitize_title_with_config("../../evil", &config);
        assert_eq!(sanitized, "../../evil", "the filter did not strip anything");
        assert_eq!(
            plain_file_component(&sanitized),
            None,
            "so the component check is what has to catch it"
        );
    }

    #[test]
    fn test_sanitize_basic() {
        // Use default config for testing
        let config = TitleFilterConfig {
            filters: FilterSettings::default(),
            replacements: HashMap::new(),
        };
        assert_eq!(
            sanitize_title_with_config("Normal Title", &config),
            "Normal Title"
        );
    }

    #[test]
    fn test_sanitize_invalid_chars() {
        let config = TitleFilterConfig {
            filters: FilterSettings::default(),
            replacements: HashMap::new(),
        };
        assert_eq!(
            sanitize_title_with_config("Test/Game:Full*Version?", &config),
            "Test_Game_Full_Version_"
        );
    }

    #[test]
    fn test_sanitize_japanese() {
        let config = TitleFilterConfig {
            filters: FilterSettings::default(),
            replacements: HashMap::new(),
        };
        assert_eq!(
            sanitize_title_with_config("テストゲーム", &config),
            "テストゲーム"
        );
    }

    #[test]
    fn test_sanitize_with_replacements() {
        let mut replacements = HashMap::new();
        replacements.insert("【".to_string(), "[".to_string());
        replacements.insert("】".to_string(), "]".to_string());

        let config = TitleFilterConfig {
            filters: FilterSettings::default(),
            replacements,
        };

        assert_eq!(
            sanitize_title_with_config("【Test】Game", &config),
            "[Test]Game"
        );
    }

    /// Regression test for M3 from `docs/AUDIT_2026-05-03.md`.
    ///
    /// `FILTER_CACHE` originally used `std::sync::RwLock` and called
    /// `.write().unwrap()` / `.read().unwrap()` everywhere. If any
    /// caller panicked while holding either guard, the lock became
    /// poisoned and every subsequent `sanitize_title` call panicked
    /// too — taking the whole UI thread down with it. The fix switches
    /// to `parking_lot::RwLock`, which has no poisoning concept.
    ///
    /// This is a type-level regression test: it asserts (at compile
    /// time) that `FILTER_CACHE` is `Arc<parking_lot::RwLock<…>>`. If
    /// someone reverts to `std::sync::RwLock`, this test fails to
    /// compile, which is the boundary we want to defend.
    #[test]
    fn m3_filter_cache_uses_parking_lot_rwlock() {
        let _: &Arc<parking_lot::RwLock<TitleFilterConfig>> = &*FILTER_CACHE;
    }

    /// Companion runtime smoke test: a thread that panics while
    /// holding the cache write guard does not break subsequent
    /// `sanitize_title` calls. Pre-fix, this test panics in the
    /// foreground call to `sanitize_title` because `std::sync::RwLock`
    /// poisons. Post-fix, it returns normally.
    #[test]
    fn m3_sanitize_title_resilient_to_panic_holding_cache() {
        let cache = FILTER_CACHE.clone();
        let h = std::thread::spawn(move || {
            let _g = cache.write();
            panic!("intentional poisoning attempt");
        });
        let _ = h.join(); // ignore the panic propagated by join

        let result = sanitize_title("simple_title");
        assert_eq!(
            result, "simple_title",
            "M3 fix regressed: sanitize_title broke after a thread panicked \
             while holding the cache lock"
        );
    }
}
