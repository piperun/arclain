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
//!
//! **The string is always the UTC rendering of one true instant.** The
//! consumer parsing it back treats it as UTC, so what it stores
//! (`modified_at_unix_ms` in the application facade) converts back to
//! that very instant on any machine in any zone. One rule per kind of
//! source keeps that honest:
//!
//! - **A source that records a true instant** (7z's Windows file times)
//!   is rendered in UTC directly -- see [`from_unix_seconds`].
//! - **A source that records a zone-less wall clock** (a ZIP or RAR4
//!   header's MS-DOS word), or one that *arrives* as a wall clock (a
//!   RAR5 UTC file time the unrar library has already rendered into the
//!   reader's zone; the local-time text the 7-Zip and unrar CLIs print),
//!   is interpreted in the **system local zone** -- the reading Explorer
//!   and the archivers' own UIs show for the same file -- and converted
//!   to the UTC instant that reading denotes. See [`from_zip_datetime`],
//!   [`from_msdos`] and [`from_cli_text`], all through
//!   [`wall_clock_to_utc_seconds`].
//!
//! A wall clock is resolved against the zone deterministically, never by
//! panicking on a daylight-saving edge: a reading the zone's fall-back
//! hour makes ambiguous takes the **earlier** of its two instants, and a
//! reading the spring-forward gap erases entirely is extrapolated with
//! the offset in force **before** the gap (see
//! [`wall_clock_to_utc_seconds`] for the exact rule and its bounded
//! fallback).
//!
//! Displaying a stored instant is the inverse, owned by the UI: it
//! renders the UTC instant back into the viewer's local zone. This module
//! only ever produces the UTC wire form.

/// Renders one civil date-time in the shape described above.
fn civil(year: u32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> String {
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

/// Resolves one zone-less wall-clock reading against `zone`, returning
/// the UTC instant it denotes there as whole seconds since the Unix
/// epoch.
///
/// Daylight-saving edges make this a choice, not a lookup, and the
/// choice here is deterministic in every branch:
///
/// - An ordinary reading names exactly one instant: that instant.
/// - A reading inside the zone's fall-back hour names two instants; the
///   **earlier** one (the pre-transition offset) is taken.
/// - A reading inside the spring-forward gap names none. It is
///   extrapolated with the offset in force **before** the gap -- resolve
///   the reading one hour earlier (which lies before any gap up to an
///   hour wide) and add the hour back. For the pathological wider-than-
///   an-hour gap (historic calendar jumps, not any present-day DST
///   rule), the reading falls back to being taken as UTC verbatim --
///   still deterministic, never a panic.
///
/// `None` only at chrono's own representable-range edges (a probe
/// stepping past `NaiveDateTime`'s minimum or maximum), which no real
/// archive time reaches.
///
/// Generic over the zone so the DST branches are testable against fixed
/// IANA zones; production callers pass [`chrono::Local`].
fn wall_clock_to_utc_seconds<Tz: chrono::TimeZone>(
    zone: &Tz,
    wall: chrono::NaiveDateTime,
) -> Option<i64> {
    use chrono::LocalResult;

    let resolved = match zone.from_local_datetime(&wall) {
        LocalResult::Single(instant) => instant,
        LocalResult::Ambiguous(earliest, _latest) => earliest,
        LocalResult::None => {
            let hour = chrono::TimeDelta::hours(1);
            let before_gap = wall.checked_sub_signed(hour)?;
            match zone.from_local_datetime(&before_gap) {
                LocalResult::Single(instant) => instant.checked_add_signed(hour)?,
                LocalResult::Ambiguous(earliest, _latest) => earliest.checked_add_signed(hour)?,
                LocalResult::None => return Some(wall.and_utc().timestamp()),
            }
        }
    };
    Some(resolved.timestamp())
}

/// Renders one wall-clock reading per the module policy: interpreted in
/// the system local zone, converted to UTC, rendered in the fixed-width
/// shape.
///
/// `None` for fields that name no calendar date-time at all (a
/// February 30th, a 25th hour): such a reading denotes no instant, and
/// rendering it anyway would hand consumers a string that parses to an
/// arbitrary one.
fn from_wall_clock(
    year: u32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> Option<String> {
    let wall = chrono::NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, month, day)?
        .and_hms_opt(hour, minute, second)?;
    from_unix_seconds(wall_clock_to_utc_seconds(&chrono::Local, wall)?)
}

/// Renders a [`zip::DateTime`], the decoded form of the date-time a ZIP
/// header carries: a wall clock with no zone attached, taken through the
/// module's local-zone policy.
///
/// `None` for a header whose fields pass the reader's per-field range
/// checks but name no real date (a February 30th): no instant exists for
/// it to denote.
pub(super) fn from_zip_datetime(value: zip::DateTime) -> Option<String> {
    from_wall_clock(
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
/// The word is a wall clock with no zone attached, so it takes the
/// module's local-zone policy. What that means depends on where the word
/// came from:
///
/// - A ZIP header, and a RAR4 header, genuinely store such a word. Its
///   local-zone reading is the reading Explorer and WinRAR show for the
///   same entry, and the stored instant is that reading's UTC
///   conversion.
/// - A RAR5 header stores a UTC file time instead; the unrar library
///   manufactures this word from it in *the reader's* local zone before
///   this crate ever sees it. Interpreting the word back through that
///   same zone cancels the library's conversion, so the stored instant
///   is the very one the header records.
pub(super) fn from_msdos(packed: u32) -> Option<String> {
    let date_part = (packed >> 16) as u16;
    let time_part = (packed & 0xFFFF) as u16;
    zip::DateTime::try_from_msdos(date_part, time_part)
        .ok()
        .and_then(from_zip_datetime)
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
/// Both tools print the entry's time as a **local-zone wall clock**, so
/// the parsed fields take the module's local-zone policy -- which, since
/// the tool rendered into the same zone this converts back out of,
/// recovers the instant the tool started from.
///
/// The seconds field is optional: unrar's short form prints `hh:mm`.
/// Sub-second precision is dropped, which this shape cannot carry anyway.
/// Field validation is the calendar's own: a value with a month of 13,
/// or a day no month has (February 30th), names no instant and yields
/// `None`.
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

    from_wall_clock(year, month, day, hour, minute, second)
}

/// Renders an absolute instant, given as whole seconds since the Unix
/// epoch, in UTC.
///
/// The one renderer every converter above funnels into, and the direct
/// entry point for formats that record a true instant rather than a
/// zone-less wall clock (7z's Windows file times, for one): rendered in
/// UTC, the string converts back to the very instant the archive
/// recorded, on any machine in any zone. A viewer's local clock may read
/// differently from this by its UTC offset; the display layer, not this
/// module, owns rendering the instant back into that zone.
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

/// Expectation builders for the tests of this module and of every backend
/// that reports times through it. Test-only; derives through chrono alone
/// (never through this module's own rendering) so an assertion built here
/// checks the production pipeline against an independent construction.
#[cfg(test)]
pub(crate) mod test_support {
    /// The wire string this module's policy assigns to a wall-clock
    /// reading: the reading interpreted in the system local zone,
    /// converted to UTC, rendered in the fixed-width shape.
    ///
    /// Deriving the expectation through `chrono::Local` is what keeps an
    /// assertion built on this zone-stable: it encodes "the local
    /// interpretation of this wall clock", not one machine's answer.
    /// Callers must pick wall times far from any zone's DST transitions
    /// (those land around 00:00-04:00 local, in spring and autumn) --
    /// `earliest()` still resolves an ambiguous reading exactly as the
    /// production converter does, but a nonexistent one panics the test
    /// rather than silently encoding the gap fallback.
    pub(crate) fn utc_wire_string_of_local_wall(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
    ) -> String {
        use chrono::TimeZone as _;
        chrono::Local
            .with_ymd_and_hms(year, month, day, hour, minute, second)
            .earliest()
            .expect("test wall times are chosen far from any DST transition")
            .with_timezone(&chrono::Utc)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wall-clock reading for the resolver tests, in fields.
    fn wall(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
    ) -> chrono::NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(year, month, day)
            .expect("test dates are valid")
            .and_hms_opt(hour, minute, second)
            .expect("test times are valid")
    }

    /// The shape the application facade parses `ArchiveEntry::modified`
    /// back out of: date and time separated by a space, every field
    /// zero-padded to a fixed width, no zone marker -- and, for a
    /// wall-clock source, the *instant* policy on top of the shape: the
    /// reading is interpreted in the system local zone and stored as the
    /// UTC instant it denotes there.
    #[test]
    fn every_converter_renders_the_same_fixed_width_shape() {
        assert_eq!(civil(2026, 5, 4, 13, 37, 20), "2026-05-04 13:37:20");
        assert_eq!(civil(999, 1, 2, 3, 4, 5), "0999-01-02 03:04:05");
        // 2026-05-04 13:37:20 packed MS-DOS: year 46 past 1980, seconds
        // halved. A wall clock, so what is stored is its local-zone
        // instant rendered in UTC -- not the fields as given.
        let packed = ((46u32 << 9 | 5 << 5 | 4) << 16) | (13 << 11 | 37 << 5 | 10);
        assert_eq!(
            from_msdos(packed),
            Some(test_support::utc_wire_string_of_local_wall(
                2026, 5, 4, 13, 37, 20
            ))
        );
        assert_eq!(
            from_unix_seconds(1_777_894_640).as_deref(),
            Some("2026-05-04 11:37:20")
        );
    }

    /// An ordinary reading -- nowhere near a transition -- names exactly
    /// one instant in its zone. Pinned against a fixed IANA zone with a
    /// hand-computed expectation, so the resolver is checked against
    /// independent arithmetic rather than against chrono itself.
    #[test]
    fn an_ordinary_wall_clock_resolves_to_its_zones_single_instant() {
        // 2026-05-04 13:37:20 in America/New_York is EDT (UTC-4):
        // 2026-05-04 17:37:20 UTC. Days 1970-01-01..2026-05-04 = 20577,
        // so 20577 * 86400 + 17h37m20s = 1_777_916_240.
        assert_eq!(
            wall_clock_to_utc_seconds(&chrono_tz::America::New_York, wall(2026, 5, 4, 13, 37, 20)),
            Some(1_777_916_240)
        );
    }

    /// A reading inside the fall-back hour names two instants; the
    /// resolver must take the earlier one, deterministically.
    #[test]
    fn an_ambiguous_fall_back_wall_clock_resolves_to_the_earlier_instant() {
        // America/New_York repeats 01:00-02:00 on 2026-11-01 (EDT
        // UTC-4 becomes EST UTC-5 at 02:00). 01:30:00 therefore names
        // both 05:30 UTC (still EDT) and 06:30 UTC (EST); the earlier is
        // 2026-11-01 05:30:00 UTC. Days 1970-01-01..2026-11-01 = 20758,
        // so 20758 * 86400 + 5h30m = 1_793_511_000.
        assert_eq!(
            wall_clock_to_utc_seconds(&chrono_tz::America::New_York, wall(2026, 11, 1, 1, 30, 0)),
            Some(1_793_511_000)
        );
    }

    /// A reading inside the spring-forward gap names no instant; the
    /// resolver must extrapolate it with the pre-gap offset,
    /// deterministically, rather than dropping the time or panicking.
    #[test]
    fn a_nonexistent_spring_forward_wall_clock_extrapolates_the_pre_gap_offset() {
        // America/New_York skips 02:00-03:00 on 2026-03-08 (EST UTC-5
        // becomes EDT UTC-4 at 02:00). 02:30:00 exists on no clock; with
        // the pre-gap EST offset it denotes 2026-03-08 07:30:00 UTC.
        // Days 1970-01-01..2026-03-08 = 20520, so
        // 20520 * 86400 + 7h30m = 1_772_955_000.
        assert_eq!(
            wall_clock_to_utc_seconds(&chrono_tz::America::New_York, wall(2026, 3, 8, 2, 30, 0)),
            Some(1_772_955_000)
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
