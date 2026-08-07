//! The one place [`crate::ArchiveEntry::modified`] strings are produced.
//!
//! Every backend reports an entry's modification time as the fixed-width,
//! zero-padded `"YYYY-MM-DD hh:mm:ss"` string these helpers render, and
//! consumers recover a real timestamp by parsing that exact shape. A
//! backend that invents its own layout still *looks* populated while
//! parsing back to nothing downstream, so the layout is written once here
//! rather than once per backend, and each archive format's own on-disk
//! encoding gets a converter into it.

/// Renders one civil date-time in the shape described above.
fn civil(year: u32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> String {
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

/// Renders a [`zip::DateTime`], the decoded form of the date-time a ZIP
/// header carries.
pub(super) fn from_zip_datetime(value: zip::DateTime) -> String {
    civil(
        u32::from(value.year()),
        u32::from(value.month()),
        u32::from(value.day()),
        u32::from(value.hour()),
        u32::from(value.minute()),
        u32::from(value.second()),
    )
}

/// Renders an MS-DOS packed date-time -- the date in the high half, the
/// time in the low half -- as RAR headers carry it.
///
/// The encoding is the same one a ZIP header uses, so it is decoded with
/// the same reader ZIP times already go through: that reader validates
/// every field, so a header with a garbage or absent time (which RAR
/// reports as an all-zero word) yields `None` rather than a nonsense date
/// that would read as a real one.
///
/// Like a ZIP header's, the recorded fields are local wall-clock with no
/// zone attached; they are rendered exactly as recorded.
pub(super) fn from_msdos(packed: u32) -> Option<String> {
    let date_part = (packed >> 16) as u16;
    let time_part = (packed & 0xFFFF) as u16;
    zip::DateTime::try_from_msdos(date_part, time_part)
        .ok()
        .map(from_zip_datetime)
}

/// Renders an absolute instant, given as whole seconds since the Unix
/// epoch, in UTC.
///
/// Formats that record a true instant rather than a zone-less wall clock
/// (7z's Windows file times, for one) are rendered in UTC so the string
/// converts back to the very instant the archive recorded, on any machine
/// in any zone. A viewer's local clock may therefore read differently
/// from this by its UTC offset; that difference is the offset, not a
/// wrong time.
///
/// `None` for an instant outside the years this shape can express.
pub(super) fn from_unix_seconds(seconds: i64) -> Option<String> {
    use chrono::{Datelike, Timelike};

    let moment = chrono::DateTime::from_timestamp(seconds, 0)?;
    let year = u32::try_from(moment.year()).ok()?;
    Some(civil(
        year,
        moment.month(),
        moment.day(),
        moment.hour(),
        moment.minute(),
        moment.second(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape the application facade parses `ArchiveEntry::modified`
    /// back out of: date and time separated by a space, every field
    /// zero-padded to a fixed width, no zone marker.
    #[test]
    fn every_converter_renders_the_same_fixed_width_shape() {
        assert_eq!(civil(2026, 5, 4, 13, 37, 20), "2026-05-04 13:37:20");
        assert_eq!(civil(999, 1, 2, 3, 4, 5), "0999-01-02 03:04:05");
        // 2026-05-04 13:37:20 packed MS-DOS: year 46 past 1980, seconds
        // halved.
        let packed = ((46u32 << 9 | 5 << 5 | 4) << 16) | (13 << 11 | 37 << 5 | 10);
        assert_eq!(from_msdos(packed).as_deref(), Some("2026-05-04 13:37:20"));
        assert_eq!(
            from_unix_seconds(1_777_894_640).as_deref(),
            Some("2026-05-04 11:37:20")
        );
    }

    /// An MS-DOS word a header can genuinely carry -- all zeroes for "no
    /// time recorded" -- must not render as a date.
    #[test]
    fn from_msdos_rejects_an_absent_or_invalid_time_word() {
        assert_eq!(from_msdos(0), None);
        // Month 13, which no calendar has.
        assert_eq!(from_msdos((13u32 << 5 | 4) << 16), None);
    }

    /// A pre-Gregorian instant has no representation in this shape, and
    /// must not be rendered as a negative or truncated year.
    #[test]
    fn from_unix_seconds_rejects_instants_before_year_zero() {
        assert_eq!(from_unix_seconds(-70_000_000_000_000), None);
        assert_eq!(
            from_unix_seconds(-1).as_deref(),
            Some("1969-12-31 23:59:59")
        );
    }
}
