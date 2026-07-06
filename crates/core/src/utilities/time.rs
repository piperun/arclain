//! Time utilities.
//!
//! `unix_seconds()` is a panic-free replacement for the `SystemTime::now()
//! .duration_since(UNIX_EPOCH).unwrap()` pattern that was scattered across
//! the codebase. The `unwrap()` form panics if the system clock is set
//! before the Unix epoch (rare but possible after BIOS battery failure).
//! Returning 0 in that case keeps callers running rather than crashing
//! the whole UI thread or a checksum operation.

use std::time::{SystemTime, UNIX_EPOCH};

/// Current Unix time in seconds, or 0 if the clock is before 1970.
///
/// Use this in hot paths and persistence-style timestamping where a
/// momentary clock anomaly should not crash the application.
pub fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Same as [`unix_seconds`] but returns `i64` for callers that need to
/// store into SQLite columns or other signed-integer slots.
pub fn unix_seconds_i64() -> i64 {
    unix_seconds() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity: the helper returns *something* close to "now" and doesn't
    /// panic. Real broken-clock testing requires mocking SystemTime,
    /// which the std lib doesn't expose; the value of this test is
    /// catching regressions where a future refactor reintroduces
    /// `.unwrap()` (which would panic on misconfigured CI runners with
    /// a wrong clock).
    #[test]
    fn unix_seconds_returns_recent_value() {
        let s = unix_seconds();
        // 2026-01-01 in unix seconds. If `s` is below this, either the
        // helper returned its 0 fallback (which means the clock is
        // pathologically wrong, the test will surface it) or the system
        // clock is genuinely set far in the past.
        assert!(
            s >= 1_767_225_600,
            "unix_seconds returned {} (< 2026-01-01)",
            s
        );
        // Sanity upper bound: year 2100.
        assert!(s < 4_102_444_800, "unix_seconds returned {} (>= 2100)", s);
    }

    #[test]
    fn unix_seconds_i64_matches_unsigned() {
        let u = unix_seconds();
        let i = unix_seconds_i64();
        // They're sampled separately so allow up to 2 seconds drift.
        assert!((u as i64 - i).abs() <= 2);
    }
}
