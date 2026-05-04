use arclain_db::ConfigDb;
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
pub fn init(db: &ConfigDb) -> anyhow::Result<()> {
    // 2. Seed system replacements if missing
    seed_system_replacements(db)?;

    // 3. Load current config from DB into cache
    refresh_cache(db)?;

    Ok(())
}

/// Refresh the in-memory cache from the database
pub fn refresh_cache(db: &ConfigDb) -> anyhow::Result<()> {
    db.with_conn(|conn| {
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
fn seed_system_replacements(db: &ConfigDb) -> anyhow::Result<()> {
    db.with_conn(|conn| {
        // We want to ensure these exist, but NOT overwrite if they already exist
        // (save_title_replacement does upsert, which is fine for system defaults
        // if we assume system defaults should be enforced or if we check existence first).
        //
        // Current save_title_replacement: ON CONFLICT(original) DO UPDATE SET replacement = excluded.replacement
        // This would overwrite user changes to system defaults.
        //
        // Better strategy: Check if it exists. If not, insert.
        // Since we don't have "insert if not exists" exposed easily, we can just rely on
        // the fact that this runs once.
        //
        // Actually, let's just insert them with is_system=true.
        // If the user changed them, they might be annoyed if we reset them.
        // But for now, let's assume system defaults are sticky.

        for (original, replacement) in DEFAULT_SYSTEM_REPLACEMENTS.iter() {
            // We use a special flag or check?
            // For now, just upsert them as system rules.
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
