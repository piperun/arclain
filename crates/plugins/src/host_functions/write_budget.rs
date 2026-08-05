//! Per-instance admission budget for metadata writes.
//!
//! Rate-limiting state, not metadata-store code: the budget only
//! decides whether one more guest-emitted write may proceed, bounding
//! write rate, distinct product ids, and accepted bytes per session.

use std::collections::HashSet;
use std::time::{Duration, Instant};

const MAX_METADATA_WRITES_PER_MINUTE: usize = 120;
const MAX_METADATA_DISTINCT_IDS_PER_SESSION: usize = 1024;
const MAX_METADATA_BYTES_PER_SESSION: usize = 64 * 1024 * 1024;
const METADATA_WRITE_WINDOW: Duration = Duration::from_secs(60);

pub(super) struct MetadataWriteBudget {
    window_started: Instant,
    writes_in_window: usize,
    accepted_bytes: usize,
    distinct_ids: HashSet<String>,
}

impl Default for MetadataWriteBudget {
    fn default() -> Self {
        Self {
            window_started: Instant::now(),
            writes_in_window: 0,
            accepted_bytes: 0,
            distinct_ids: HashSet::new(),
        }
    }
}

impl MetadataWriteBudget {
    pub(super) fn admit(&mut self, id: &str, bytes: usize) -> bool {
        self.admit_at(id, bytes, Instant::now())
    }

    fn admit_at(&mut self, id: &str, bytes: usize, now: Instant) -> bool {
        if now
            .checked_duration_since(self.window_started)
            .is_some_and(|elapsed| elapsed >= METADATA_WRITE_WINDOW)
        {
            self.window_started = now;
            self.writes_in_window = 0;
        }
        if self.writes_in_window >= MAX_METADATA_WRITES_PER_MINUTE {
            return false;
        }
        if !self.distinct_ids.contains(id)
            && self.distinct_ids.len() >= MAX_METADATA_DISTINCT_IDS_PER_SESSION
        {
            return false;
        }
        let Some(next_bytes) = self.accepted_bytes.checked_add(bytes) else {
            return false;
        };
        if next_bytes > MAX_METADATA_BYTES_PER_SESSION {
            return false;
        }

        self.writes_in_window += 1;
        self.accepted_bytes = next_bytes;
        self.distinct_ids.insert(id.to_owned());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_write_budget_bounds_rate_distinct_ids_and_session_bytes() {
        let start = std::time::Instant::now();
        let mut rate = MetadataWriteBudget::default();
        for index in 0..MAX_METADATA_WRITES_PER_MINUTE {
            assert!(rate.admit_at(&format!("dlsite:RJ{index:06}"), 1, start));
        }
        assert!(!rate.admit_at("dlsite:RJ999999", 1, start));

        let mut distinct = MetadataWriteBudget::default();
        for index in 0..MAX_METADATA_DISTINCT_IDS_PER_SESSION {
            assert!(distinct.admit_at(
                &format!("dlsite:RJ{index:06}"),
                1,
                start + std::time::Duration::from_secs(index as u64 * 61),
            ));
        }
        assert!(
            !distinct.admit_at(
                "dlsite:one-too-many",
                1,
                start
                    + std::time::Duration::from_secs(
                        MAX_METADATA_DISTINCT_IDS_PER_SESSION as u64 * 61,
                    ),
            )
        );

        let mut bytes = MetadataWriteBudget::default();
        assert!(bytes.admit_at("dlsite:A", MAX_METADATA_BYTES_PER_SESSION, start));
        assert!(!bytes.admit_at("dlsite:A", 1, start + std::time::Duration::from_secs(61),));
    }
}
