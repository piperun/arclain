//! The one place [`crate::ArchiveEntry::modified`] strings are produced.
//!
//! Every backend reports an entry's modification time as the fixed-width,
//! zero-padded `"YYYY-MM-DD hh:mm:ss"` string these helpers render, and
//! consumers recover a real timestamp by parsing that exact shape. A
//! backend that invents its own layout still *looks* populated while
//! parsing back to nothing downstream, so the layout is written once here
//! rather than once per backend, and every source a backend can read a
//! time from gets a converter into it: an archive format's own on-disk
//! encoding for the native tiers, and an archiver's printed output for
//! the CLI tiers.

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
/// time in the low half.
///
/// Decoded with the reader ZIP times already go through, which validates
/// every field, so an absent time (an all-zero word) or a garbage one
/// yields `None` rather than a nonsense date that would read as a real
/// one.
///
/// **The fields are a wall clock with no zone attached, and this renders
/// them exactly as given.** Whether that faithfully describes the archive
/// depends on where the word came from:
///
/// - A ZIP header, and a RAR4 header, genuinely store such a word. It
///   means the same thing on every machine.
/// - A RAR5 header stores a UTC file time instead; the unrar library
///   converts it into a word like this using *the reader's* local zone
///   before this backend ever sees it. Those entries are therefore
///   zone-dependent -- the conversion has already happened, and it is not
///   reversible from here.
pub(super) fn from_msdos(packed: u32) -> Option<String> {
    let date_part = (packed >> 16) as u16;
    let time_part = (packed & 0xFFFF) as u16;
    zip::DateTime::try_from_msdos(date_part, time_part)
        .ok()
        .map(from_zip_datetime)
}

/// Normalizes a date-time an archiver's own command-line tool printed.
///
/// The CLI tiers do not decode a header themselves -- they read back text
/// their tool already formatted, which is *close* to this shape but not
/// it: 7-Zip prints `2026-05-04 13:37:20.0000000` and unrar prints
/// `2026-05-04 13:37:20,000000000`, both carrying a sub-second remainder
/// this shape has no room for, behind a different separator each. Passing
/// either through verbatim yields a string consumers reject outright,
/// leaving the entry dateless with nothing reporting a problem -- so the
/// text is parsed into fields and re-rendered here rather than trimmed,
/// and anything that does not parse becomes `None` honestly.
///
/// The seconds field is optional: unrar's short form prints `hh:mm`.
/// Sub-second precision is dropped, which this shape cannot carry anyway.
pub(super) fn from_cli_text(value: &str) -> Option<String> {
    let (date_part, time_part) = value.trim().split_once(' ')?;

    let mut date_fields = date_part.split('-');
    let year: u32 = date_fields.next()?.parse().ok()?;
    let month: u32 = date_fields.next()?.parse().ok()?;
    let day: u32 = date_fields.next()?.parse().ok()?;
    if date_fields.next().is_some() {
        return None;
    }

    // Cut the sub-second remainder off whichever separator introduced it.
    let time_part = time_part.trim().split([',', '.']).next()?;
    let mut time_fields = time_part.split(':');
    let hour: u32 = time_fields.next()?.parse().ok()?;
    let minute: u32 = time_fields.next()?.parse().ok()?;
    let second: u32 = match time_fields.next() {
        Some(field) => field.parse().ok()?,
        None => 0,
    };
    if time_fields.next().is_some() {
        return None;
    }

    // Exactly the ranges the consuming parser accepts, so nothing this
    // returns can be rejected downstream.
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    Some(civil(year, month, day, hour, minute, second))
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

    /// An instant before year zero has no representation in this shape,
    /// and must not be rendered as a negative or truncated year.
    #[test]
    fn from_unix_seconds_rejects_instants_before_year_zero() {
        assert_eq!(from_unix_seconds(-70_000_000_000_000), None);
    }

    /// Merely being before the *Unix* epoch is not out of range, though:
    /// archives predating 1970 exist, and must floor to the earlier
    /// second rather than truncating toward the epoch.
    #[test]
    fn from_unix_seconds_still_renders_instants_before_the_unix_epoch() {
        assert_eq!(
            from_unix_seconds(-1).as_deref(),
            Some("1969-12-31 23:59:59")
        );
    }
}
