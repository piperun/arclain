//! Password matching helper for auto-detecting passwords from rules.
//!
//! This module provides password matching logic that was previously in ConfigStore.
//! It takes a list of PassRule and matches against archive names/file entries.

use crate::utilities::dlsite::detect_dlsite_code;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A password matching rule
#[derive(Clone, Serialize, Deserialize)]
pub struct PassRule {
    pub name: String,
    pub pattern: String,
    pub password: String,
    pub priority: u32,
    pub enabled: bool,
}

// Custom Debug implementation to avoid logging passwords
impl std::fmt::Debug for PassRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PassRule")
            .field("name", &self.name)
            .field("pattern", &self.pattern)
            .field("password", &"[REDACTED]")
            .field("priority", &self.priority)
            .field("enabled", &self.enabled)
            .finish()
    }
}

impl PassRule {
    pub fn to_regex(&self) -> Option<Regex> {
        Regex::new(&self.pattern).ok()
    }
}

/// Find the first matching password from a list of rules.
///
/// Matches against both the archive filename and file entries inside.
/// Rules are sorted by priority (descending) before matching.
pub fn auto_password_for(
    rules: &[PassRule],
    archive_path: Option<&str>,
    filenames: &[String],
) -> Option<String> {
    let mut sorted_rules: Vec<&PassRule> = rules.iter().filter(|r| r.enabled).collect();
    sorted_rules.sort_by_key(|r| std::cmp::Reverse(r.priority));

    for rule in sorted_rules {
        if let Some(re) = rule.to_regex() {
            // First check archive filename if provided
            if let Some(archive) = archive_path {
                // Extract just the filename from the full path
                let archive_name = Path::new(archive)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(archive);

                if re.is_match(archive_name) {
                    return Some(rule.password.clone());
                }
            }

            // Also check internal file paths for backwards compatibility
            if filenames.iter().any(|f| re.is_match(f)) {
                return Some(rule.password.clone());
            }
        }
    }
    None
}

/// Derive a regex pattern from an archive filename suitable for an
/// auto-saved password rule.
///
/// The previous behavior wrote `regex::escape(filename)` — i.e. a
/// pattern that matches that one exact filename and nothing else. So
/// a user who unlocked `Foo [RJ100001] v1.zip` would get re-prompted
/// when they later opened `Foo [RJ100001] v2.zip` (same product,
/// same password, different filename). The common case for arclain
/// is "I have N archives from the same source, all share one
/// password" — the literal-filename pattern is the worst possible
/// fit for that.
///
/// New heuristic, in priority order:
///
/// 1. **DLsite-style product code** (RJ/VJ/BJ + digits). Most
///    specific cross-archive identifier — same code means same
///    product across versions, mirrors, re-encodings, etc. Pattern
///    is case-insensitive so `rj100001` and `RJ100001` both fire.
/// 2. **Leading bracket-enclosed maker name** (`[Maker-Name] ...`).
///    Common DLsite/scene convention for tagging the publisher;
///    same bracket usually means the same passphrase across an
///    entire catalog.
/// 3. **Literal filename escape** (the old behavior). Falls back
///    when neither a product code nor a leading bracket is present.
///    User can manually broaden the pattern in the Password Rules
///    settings if they want it to match more than one archive.
pub fn derive_pattern_for(filename: &str) -> String {
    if let Some(code) = detect_dlsite_code(filename) {
        // Case-insensitive literal-code match. The code is
        // alphanumeric so escape is technically redundant, but
        // we run it anyway in case the upstream detector ever
        // returns a value with metachars (e.g. a future variant).
        return format!("(?i){}", regex::escape(&code));
    }

    if let Some(maker) = regex::Regex::new(r"^\[([^\]]+)\]")
        .ok()
        .and_then(|re| re.captures(filename))
        .and_then(|caps| caps.get(1))
    {
        return format!(r"^\[{}\]", regex::escape(maker.as_str()));
    }

    regex::escape(filename)
}

/// Re-derive broader patterns for auto-saved rules that still carry the
/// old one-archive-only `regex::escape(filename)` pattern (saved before
/// `derive_pattern_for` existed). Returns `Some(upgraded)` if any rule
/// changed, `None` if nothing matched — so the caller can skip the DB
/// write entirely when there's nothing to do.
///
/// A rule is upgraded **only** when it bears the exact auto-saved
/// fingerprint that `save_password_rule_from_archive` produced:
///
/// - name is `"Auto-saved: <filename>"`, **and**
/// - pattern equals `regex::escape(<filename>)` (i.e. still literal).
///
/// That double check means a rule the user renamed, hand-broadened, or
/// crafted from scratch is never touched — we only ever rewrite rules
/// we can prove we auto-generated narrowly. Order is preserved, so the
/// caller can zip old/new to count changes.
///
/// Idempotent: once broadened, the pattern no longer equals the literal
/// escape, so a second pass leaves it alone and returns `None`.
pub fn upgrade_auto_saved_rules(rules: &[PassRule]) -> Option<Vec<PassRule>> {
    const PREFIX: &str = "Auto-saved: ";
    let mut changed = false;
    let upgraded: Vec<PassRule> = rules
        .iter()
        .map(|rule| {
            if let Some(filename) = rule.name.strip_prefix(PREFIX) {
                if rule.pattern == regex::escape(filename) {
                    let broadened = derive_pattern_for(filename);
                    if broadened != rule.pattern {
                        changed = true;
                        return PassRule {
                            pattern: broadened,
                            ..rule.clone()
                        };
                    }
                }
            }
            rule.clone()
        })
        .collect();
    changed.then_some(upgraded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auto_saved(filename: &str) -> PassRule {
        PassRule {
            name: format!("Auto-saved: {}", filename),
            pattern: regex::escape(filename),
            password: "pw".to_string(),
            priority: 10,
            enabled: true,
        }
    }

    #[test]
    fn test_password_matching_by_archive_name() {
        let rules = vec![PassRule {
            name: "Test".to_string(),
            pattern: r"test\.zip".to_string(),
            password: "secret".to_string(),
            priority: 10,
            enabled: true,
        }];

        let result = auto_password_for(&rules, Some("C:\\foo\\test.zip"), &[]);
        assert_eq!(result, Some("secret".to_string()));
    }

    #[test]
    fn test_password_matching_disabled_rule() {
        let rules = vec![PassRule {
            name: "Disabled".to_string(),
            pattern: r"test\.zip".to_string(),
            password: "secret".to_string(),
            priority: 10,
            enabled: false,
        }];

        let result = auto_password_for(&rules, Some("test.zip"), &[]);
        assert_eq!(result, None);
    }

    // -------- derive_pattern_for --------

    /// DLsite RJ code wins over everything else — it's the strongest
    /// cross-archive identifier. The derived pattern matches the
    /// code only, case-insensitively, so different filenames with
    /// the same code all fire the same rule.
    #[test]
    fn derive_pattern_picks_dlsite_code() {
        let pattern = derive_pattern_for("Some Title [RJ100001] v2.zip");
        assert_eq!(pattern, "(?i)RJ100001");

        // Round-trip: the derived pattern must actually match the
        // source filename it was derived from, plus same-code
        // variants with totally different surrounding text.
        let re = Regex::new(&pattern).unwrap();
        assert!(re.is_match("Some Title [RJ100001] v2.zip"));
        assert!(re.is_match("rj100001-translated.7z")); // lowercase + suffix
        assert!(re.is_match("[Crew] Repack rj100001 final.rar"));
        assert!(!re.is_match("Some Title [RJ100002] v2.zip")); // different code
    }

    /// VJ / BJ codes work the same as RJ — same detector.
    #[test]
    fn derive_pattern_picks_vj_and_bj_codes() {
        assert_eq!(derive_pattern_for("Game [VJ500000].zip"), "(?i)VJ500000");
        assert_eq!(derive_pattern_for("App [BJ200000].zip"), "(?i)BJ200000");
    }

    /// No code present, but a leading bracket — fall through to
    /// maker-bracket pattern. Matches any archive that *starts with*
    /// the same bracket.
    #[test]
    fn derive_pattern_falls_back_to_leading_bracket() {
        let pattern = derive_pattern_for("[Crew Name] Some Title.zip");
        assert_eq!(pattern, r"^\[Crew Name\]");

        let re = Regex::new(&pattern).unwrap();
        assert!(re.is_match("[Crew Name] Some Title.zip"));
        assert!(re.is_match("[Crew Name] Another Title.7z"));
        assert!(!re.is_match("Some Title [Crew Name].zip")); // bracket not leading
        assert!(!re.is_match("[Other Crew] Title.zip"));
    }

    /// No code, no leading bracket — fall through to literal filename
    /// escape. Same as the pre-refactor behavior: matches that one
    /// archive only.
    #[test]
    fn derive_pattern_literal_fallback() {
        let pattern = derive_pattern_for("plain-archive.zip");
        // regex::escape converts metachars; `-` and alphanumerics
        // pass through unchanged but `.` becomes `\.`.
        assert_eq!(pattern, r"plain\-archive\.zip");

        let re = Regex::new(&pattern).unwrap();
        assert!(re.is_match("plain-archive.zip"));
        assert!(!re.is_match("other-archive.zip"));
    }

    /// Defensive: a filename with regex metachars in it shouldn't
    /// produce an invalid pattern.
    #[test]
    fn derive_pattern_escapes_metachars_in_fallback() {
        let pattern = derive_pattern_for("weird (file) [v1.0+].zip");
        // No DLsite code, leading char is `w` not `[`, so falls
        // through to literal escape — should not panic, must
        // produce a valid compilable regex.
        let re = Regex::new(&pattern).expect("escaped pattern must compile");
        assert!(re.is_match("weird (file) [v1.0+].zip"));
    }

    /// Edge case: filename with BOTH a leading bracket AND a code.
    /// Code wins (priority 1).
    #[test]
    fn derive_pattern_code_wins_over_bracket() {
        let pattern = derive_pattern_for("[Crew] Title [RJ100003] v1.zip");
        assert_eq!(pattern, "(?i)RJ100003");
    }

    // -------- upgrade_auto_saved_rules --------

    /// An auto-saved rule whose filename carries an RJ code gets its
    /// literal pattern broadened to the case-insensitive code match.
    #[test]
    fn upgrade_broadens_rj_coded_auto_saved_rule() {
        let rules = vec![auto_saved("Some Title [RJ100001] v1.zip")];
        let upgraded = upgrade_auto_saved_rules(&rules).expect("should change");
        assert_eq!(upgraded[0].pattern, "(?i)RJ100001");
        // Everything else preserved.
        assert_eq!(upgraded[0].name, rules[0].name);
        assert_eq!(upgraded[0].password, rules[0].password);
        assert_eq!(upgraded[0].priority, rules[0].priority);
        assert_eq!(upgraded[0].enabled, rules[0].enabled);
    }

    /// A plain filename re-derives to the same literal escape, so the
    /// rule is unchanged — and with nothing else to change, the whole
    /// pass returns None.
    #[test]
    fn upgrade_leaves_plain_filename_rule_untouched() {
        let rules = vec![auto_saved("plain-archive.zip")];
        assert!(upgrade_auto_saved_rules(&rules).is_none());
    }

    /// A rule the user renamed (no "Auto-saved:" prefix) is never
    /// touched, even if its pattern happens to look literal.
    #[test]
    fn upgrade_skips_hand_renamed_rule() {
        let rules = vec![rule_named("My RJ pack", &regex::escape("X [RJ100001].zip"))];
        assert!(upgrade_auto_saved_rules(&rules).is_none());
    }

    /// An auto-saved rule the user already hand-broadened (pattern no
    /// longer equals the literal escape of the filename) is left alone.
    #[test]
    fn upgrade_skips_already_broadened_rule() {
        let mut r = auto_saved("Title [RJ100001] v1.zip");
        r.pattern = "(?i)RJ100001".to_string(); // user (or a prior run) broadened it
        assert!(upgrade_auto_saved_rules(&[r]).is_none());
    }

    /// Running the pass twice is a no-op the second time (idempotent).
    #[test]
    fn upgrade_is_idempotent() {
        let rules = vec![auto_saved("Game [RJ100001].zip")];
        let once = upgrade_auto_saved_rules(&rules).expect("first pass changes");
        assert!(upgrade_auto_saved_rules(&once).is_none(), "second pass is a no-op");
    }

    /// Mixed set: only the RJ-coded auto-saved rule changes; the plain
    /// one and the hand-named one ride through untouched, order kept.
    #[test]
    fn upgrade_only_touches_matching_rules() {
        let rules = vec![
            auto_saved("plain.zip"),
            auto_saved("Foo [RJ100002] v3.7z"),
            rule_named("Custom", r"^custom"),
        ];
        let upgraded = upgrade_auto_saved_rules(&rules).expect("one rule changes");
        assert_eq!(upgraded[0].pattern, regex::escape("plain.zip"));
        assert_eq!(upgraded[1].pattern, "(?i)RJ100002");
        assert_eq!(upgraded[2].pattern, r"^custom");
        let changed = upgraded
            .iter()
            .zip(&rules)
            .filter(|(a, b)| a.pattern != b.pattern)
            .count();
        assert_eq!(changed, 1);
    }

    fn rule_named(name: &str, pattern: &str) -> PassRule {
        PassRule {
            name: name.to_string(),
            pattern: pattern.to_string(),
            password: "pw".to_string(),
            priority: 10,
            enabled: true,
        }
    }
}
