use crate::shared::models::file_entry::FileEntry;
use arclain_app::archive::{ArchiveEntryDto, EntryKind};
use tracing::error;

pub fn format_duration(duration: std::time::Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        format!(
            "{:02}:{:02}:{:02}",
            seconds / 3600,
            (seconds % 3600) / 60,
            seconds % 60
        )
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    if unit_idx == 0 {
        format!("{} {}", size as u64, UNITS[unit_idx])
    } else {
        format!("{:.1} {}", size, UNITS[unit_idx])
    }
}

/// Civil date (year, month, day) for a count of days since the Unix
/// epoch. Howard Hinnant's `civil_from_days` (public domain) -- the exact
/// inverse of the `days_from_civil` the application facade uses to parse
/// a backend's `modified` string into
/// [`ArchiveEntryDto::modified_at_unix_ms`]. Only the out-of-chrono-range
/// fallback of [`format_modified_unix_ms`] still renders through this;
/// every representable instant goes through chrono's local conversion
/// instead.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]: Mar=0 .. Feb=11
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (year + i64::from(month <= 2), month, day)
}

/// Renders [`ArchiveEntryDto::modified_at_unix_ms`] as the
/// `"YYYY-MM-DD HH:MM:SS"` string the file list's Modified column shows,
/// or the empty string when the entry carries no timestamp.
///
/// The DTO carries a true UTC instant; the cell shows **the viewer's
/// local zone's reading of it** -- what Explorer and 7-Zip's own GUI
/// show for the same file -- in the same fixed-width shape every backend
/// reports times in. The zone conversion lives here, on the display
/// edge, and nowhere else: everything upstream of this function stores
/// and passes the instant itself.
///
/// An instant beyond chrono's representable years (an absurd `i64` a
/// corrupt or adversarial entry can put in the DTO) falls back to
/// rendering its raw UTC civil fields: no real zone means anything out
/// there, and the fallback keeps the conversion total -- deterministic
/// and panic-free -- instead of correct only for plausible dates.
pub fn format_modified_unix_ms(modified_at_unix_ms: Option<i64>) -> String {
    let Some(milliseconds) = modified_at_unix_ms else {
        return String::new();
    };
    // Euclidean division so a pre-1970 timestamp floors toward the
    // earlier second rather than truncating toward zero (which would
    // land in the wrong second, and on the wrong day at day edges).
    let seconds = milliseconds.div_euclid(1000);

    if let Some(instant) = chrono::DateTime::from_timestamp(seconds, 0) {
        use chrono::{Datelike, Timelike};
        // Field-formatted rather than strftime'd: chrono's `%Y` pads a
        // five-digit year with a `+` sign, while this column's shape is
        // plain zero-padded digits for every representable year.
        let local = instant.with_timezone(&chrono::Local);
        return format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            local.year(),
            local.month(),
            local.day(),
            local.hour(),
            local.minute(),
            local.second()
        );
    }

    let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
    let second_of_day = seconds.rem_euclid(86_400);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year,
        month,
        day,
        second_of_day / 3600,
        (second_of_day % 3600) / 60,
        second_of_day % 60
    )
}

/// Builds the file list's display row for one entry of an
/// [`arclain_app::archive::EntryPage`].
///
/// `FileEntry::path` is the row's path *relative to the folder on
/// screen*, which for a page's rows is simply the entry's name: a page
/// lists the direct children of the directory it was requested for, so
/// nothing in it is more than one segment deep. `FileEntry::archive_path`
/// carries the archive-root path selection and file operations key on.
pub fn file_entry_from_dto(dto: &ArchiveEntryDto) -> FileEntry {
    let compressed_size = dto.compressed_size.unwrap_or(0);
    let ratio = if dto.uncompressed_size > 0 {
        format!("{}%", compressed_size * 100 / dto.uncompressed_size)
    } else {
        "0%".to_string()
    };

    FileEntry {
        name: dto.name.clone(),
        path: dto.name.clone(),
        archive_path: dto.path.as_str().to_string(),
        size: format_size(dto.uncompressed_size),
        compressed: format_size(compressed_size),
        ratio,
        modified: format_modified_unix_ms(dto.modified_at_unix_ms),
        crc32: dto.crc32.clone().unwrap_or_default(),
        encrypted: dto.encrypted,
        is_folder: matches!(dto.kind, EntryKind::Directory),
    }
}

/// Log an error in a consistent format for failure cases.
/// This keeps our tests simple and ensures a single message shape.
pub fn log_failure(context: &str, message: impl std::fmt::Display) {
    error!("{}: {}", context, message);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_test::traced_test;

    // Verifies that our failure logging helper actually emits a log line we can assert on.
    #[traced_test]
    #[test]
    fn logs_on_failure() {
        log_failure("Settings", "failed to save");
        assert!(logs_contain("Settings: failed to save"));
    }

    // =========================================================================
    // format_duration
    // =========================================================================

    #[test]
    fn format_duration_zero() {
        assert_eq!(format_duration(std::time::Duration::from_secs(0)), "00:00");
    }

    #[test]
    fn format_duration_seconds_only() {
        assert_eq!(format_duration(std::time::Duration::from_secs(45)), "00:45");
    }

    #[test]
    fn format_duration_minutes_and_seconds() {
        assert_eq!(
            format_duration(std::time::Duration::from_secs(125)),
            "02:05"
        );
    }

    #[test]
    fn format_duration_exactly_one_hour() {
        assert_eq!(
            format_duration(std::time::Duration::from_secs(3600)),
            "01:00:00"
        );
    }

    #[test]
    fn format_duration_hours_minutes_seconds() {
        // 2h 30m 15s = 9015s
        assert_eq!(
            format_duration(std::time::Duration::from_secs(9015)),
            "02:30:15"
        );
    }

    // =========================================================================
    // format_size
    // =========================================================================

    #[test]
    fn format_size_zero_bytes() {
        assert_eq!(format_size(0), "0 B");
    }

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(512), "512 B");
    }

    #[test]
    fn format_size_kilobytes() {
        assert_eq!(format_size(1024), "1.0 KB");
    }

    #[test]
    fn format_size_megabytes() {
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
    }

    #[test]
    fn format_size_gigabytes() {
        assert_eq!(format_size(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn format_size_fractional_mb() {
        // 1.5 MB = 1572864 bytes
        assert_eq!(format_size(1_572_864), "1.5 MB");
    }

    // =========================================================================
    // format_modified_unix_ms
    // =========================================================================

    /// The Modified-cell string the display policy assigns to an
    /// instant: its rendering in the viewer's local zone. Derived
    /// through chrono alone (never through the code under test), so the
    /// assertion stays zone-stable -- it encodes "this instant, read on
    /// this machine's clock", not one zone's hardcoded answer.
    fn expected_local_render(unix_seconds: i64) -> String {
        chrono::DateTime::from_timestamp(unix_seconds, 0)
            .expect("test instants are representable")
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    }

    #[test]
    fn no_timestamp_renders_as_an_empty_modified_cell() {
        assert_eq!(format_modified_unix_ms(None), "");
    }

    #[test]
    fn the_epoch_renders_as_the_viewers_local_reading_of_it() {
        assert_eq!(format_modified_unix_ms(Some(0)), expected_local_render(0));
    }

    /// The DTO carries a true UTC instant; the Modified column shows the
    /// viewer's own clock's reading of that instant (what Explorer and
    /// 7-Zip's GUI show for the same file), in the same fixed-width
    /// shape every backend reports times in.
    #[test]
    fn a_stored_instant_renders_as_the_viewers_local_wall_clock() {
        // 2024-01-15 10:30:00 UTC
        assert_eq!(
            format_modified_unix_ms(Some(1_705_314_600_000)),
            expected_local_render(1_705_314_600)
        );
        // 2000-02-29 23:59:59 UTC -- a leap day, and the last second of it
        assert_eq!(
            format_modified_unix_ms(Some(951_868_799_000)),
            expected_local_render(951_868_799)
        );
        // 2100-03-01 00:00:00 UTC -- the century that is not a leap year
        assert_eq!(
            format_modified_unix_ms(Some(4_107_542_400_000)),
            expected_local_render(4_107_542_400)
        );
    }

    /// Sub-second precision is not part of the displayed format, so a
    /// timestamp inside a second floors to that second rather than
    /// rounding into the next one -- asserted zone-free by comparing
    /// against the same second's own rendering.
    #[test]
    fn sub_second_precision_floors_within_its_own_second() {
        assert_eq!(
            format_modified_unix_ms(Some(999)),
            format_modified_unix_ms(Some(0))
        );
    }

    /// Truncating division would land a pre-epoch timestamp inside the
    /// wrong second. Nothing in this workspace produces such an archive,
    /// but flooring is what makes the conversion total rather than
    /// accidentally correct only for dates after 1970.
    #[test]
    fn a_pre_epoch_timestamp_floors_to_the_earlier_second() {
        assert_eq!(
            format_modified_unix_ms(Some(-1000)),
            expected_local_render(-1)
        );
    }

    /// The timestamp arrives from a DTO built out of an archive's own
    /// bytes, so a corrupt or adversarial entry can put an arbitrary
    /// `i64` in it. Rendering one must not overflow (a debug-build panic
    /// on the render path) -- the two flooring divisions bound `days` to
    /// roughly ±10^11 before any multiply happens, which is what makes
    /// the calendar arithmetic total rather than merely untested.
    #[test]
    fn an_absurd_timestamp_renders_something_rather_than_overflowing() {
        for milliseconds in [i64::MIN, i64::MIN + 1, i64::MAX, i64::MAX - 1] {
            assert!(!format_modified_unix_ms(Some(milliseconds)).is_empty());
        }
    }

    // =========================================================================
    // file_entry_from_dto
    // =========================================================================

    fn dto(path: &str, kind: EntryKind) -> ArchiveEntryDto {
        ArchiveEntryDto {
            id: arclain_app::ids::EntryId::from_raw(1),
            path: arclain_app::archive::ArchivePath::parse(path.to_string()).unwrap(),
            name: path.rsplit('/').next().unwrap().to_string(),
            kind,
            compressed_size: Some(0),
            uncompressed_size: 0,
            modified_at_unix_ms: None,
            encrypted: false,
            crc32: None,
        }
    }

    #[test]
    fn a_file_row_carries_its_sizes_ratio_date_and_checksum() {
        let mut entry = dto("game/data/save.dat", EntryKind::File);
        entry.uncompressed_size = 2048;
        entry.compressed_size = Some(1024);
        entry.modified_at_unix_ms = Some(1_705_314_600_000);
        entry.crc32 = Some("AABBCCDD".to_string());

        let row = file_entry_from_dto(&entry);

        assert_eq!(row.name, "save.dat");
        assert_eq!(row.archive_path, "game/data/save.dat");
        assert_eq!(row.size, "2.0 KB");
        assert_eq!(row.compressed, "1.0 KB");
        assert_eq!(row.ratio, "50%");
        // 2024-01-15 10:30:00 UTC, shown on the viewer's own clock.
        assert_eq!(row.modified, expected_local_render(1_705_314_600));
        assert_eq!(row.crc32, "AABBCCDD");
        assert!(!row.is_folder);
        assert!(!row.encrypted);
    }

    #[test]
    fn a_folder_row_is_flagged_as_one_and_reports_a_zero_ratio() {
        let row = file_entry_from_dto(&dto("game/data", EntryKind::Directory));

        assert_eq!(row.name, "data");
        assert!(row.is_folder);
        assert_eq!(row.ratio, "0%");
        assert_eq!(row.modified, "");
        assert_eq!(row.crc32, "");
    }

    #[test]
    fn an_encrypted_row_keeps_its_flag_and_computes_its_ratio() {
        let mut entry = dto("secret.txt", EntryKind::File);
        entry.uncompressed_size = 100;
        entry.compressed_size = Some(80);
        entry.encrypted = true;

        let row = file_entry_from_dto(&entry);

        assert!(row.encrypted);
        assert_eq!(row.ratio, "80%");
    }

    /// A row's display path is relative to the folder on screen while
    /// `archive_path` stays archive-root-relative -- what keeps two
    /// same-named files in different folders distinct in the selection
    /// set (see `BrowserProjectionCache`'s own coverage of that bug).
    #[test]
    fn a_nested_row_shows_its_name_but_keys_on_its_archive_path() {
        let row = file_entry_from_dto(&dto("A/same.txt", EntryKind::File));

        assert_eq!(row.path, "same.txt");
        assert_eq!(row.archive_path, "A/same.txt");
    }

    /// A symlink has no pre-facade counterpart (no backend reports one),
    /// but it must never be mistaken for a folder: a folder row navigates
    /// on double-click instead of opening.
    #[test]
    fn a_symlink_row_is_not_treated_as_a_folder() {
        assert!(!file_entry_from_dto(&dto("link", EntryKind::Symlink)).is_folder);
    }
}
