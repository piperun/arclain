//! Detection of a split ("multi-part") archive set on disk: the
//! frontend-neutral mirror of `arclain_core::archive::MultiPartArchive`,
//! and the one place a frontend asks "is this file part of a set?".
//!
//! # Characterization: what this replaces
//!
//! Pre-facade, three `crates/ui` call sites each reached
//! `arclain_core::archive::MultiPartArchive::detect` directly and stored
//! the returned core value in per-tab UI state:
//!
//! - `crates/ui/src/core/arclain_app/drop_handler.rs` -- a dropped
//!   archive that is part of a set opens the merge dialog instead of
//!   opening the archive.
//! - `crates/ui/src/core/operations/archive.rs::open_archive_via_file_dialog`
//!   -- the same redirect for a file-picker selection.
//! - `crates/ui/src/shared/dialogs/merge_dialog.rs` -- held the detected
//!   core value as dialog state and rendered its `base_name`/`format`/
//!   `all_parts.len()`.
//!
//! # Why a free function rather than an `ArclainApp` method
//!
//! Detection touches the filesystem (see [`detect`]'s own doc comment on
//! exactly where), so it is *not* pure the way
//! [`crate::archive::is_archive_extension`] or `crate::analyze_url` are.
//! It is nonetheless a free function, for two reasons that both hold
//! independently:
//!
//! 1. It needs no application state: no session store, no services, no
//!    settings, no runtime handle. A method taking `&self` and never
//!    reading it would imply a coupling to application state that does
//!    not exist -- the same reason `analyze_url` is not a method.
//! 2. Both frontend call sites are *synchronous* branch points: the drop
//!    handler and the file-picker handler must decide "merge dialog or
//!    open?" within the frame that produced the path, and the file-picker
//!    caller reads its `MergeDialogState` back out of a local immediately
//!    after the call. An `async` method could not answer either of them
//!    without restructuring call sites into a later frame.
//!
//! The I/O involved is bounded and cheap -- `Path::exists` probes for
//! sibling files, no archive is opened and no archive content is read --
//! and both call sites already perform far heavier synchronous work
//! (a native file dialog, drop-event processing) on the same thread.

use std::path::{Path, PathBuf};

/// Which naming convention a detected multi-part set follows. Mirrors
/// `arclain_core::archive::MultiPartFormat` variant-for-variant.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MultiPartFormat {
    /// `name.part1.rar`, `name.part2.rar`, ...
    RarPart,
    /// `name.rar`, `name.r00`, `name.r01`, ...
    RarSequence,
    /// `name.7z.001`, `name.7z.002`, ...
    SevenZip,
    /// `name.z01`, `name.z02`, ..., `name.zip`
    ZipSplit,
    /// `name.001`, `name.002`, ...
    Generic001,
}

impl MultiPartFormat {
    /// A short human-readable description of the convention. Kept
    /// byte-identical to `arclain_core::archive::MultiPartFormat::
    /// description` so a frontend that renders this shows exactly the
    /// text it showed pre-facade (pinned by this module's own tests).
    pub fn description(self) -> &'static str {
        match self {
            Self::RarPart => "RAR multi-part (.partN.rar)",
            Self::RarSequence => "RAR sequence (.rar, .r00, .r01)",
            Self::SevenZip => "7-Zip split (.7z.001)",
            Self::ZipSplit => "ZIP split (.z01, .zip)",
            Self::Generic001 => "Generic split (.001, .002)",
        }
    }

    fn from_core(format: arclain_core::archive::MultiPartFormat) -> Self {
        match format {
            arclain_core::archive::MultiPartFormat::RarPart => Self::RarPart,
            arclain_core::archive::MultiPartFormat::RarSequence => Self::RarSequence,
            arclain_core::archive::MultiPartFormat::SevenZip => Self::SevenZip,
            arclain_core::archive::MultiPartFormat::ZipSplit => Self::ZipSplit,
            arclain_core::archive::MultiPartFormat::Generic001 => Self::Generic001,
        }
    }

    pub(crate) fn to_core(self) -> arclain_core::archive::MultiPartFormat {
        match self {
            Self::RarPart => arclain_core::archive::MultiPartFormat::RarPart,
            Self::RarSequence => arclain_core::archive::MultiPartFormat::RarSequence,
            Self::SevenZip => arclain_core::archive::MultiPartFormat::SevenZip,
            Self::ZipSplit => arclain_core::archive::MultiPartFormat::ZipSplit,
            Self::Generic001 => arclain_core::archive::MultiPartFormat::Generic001,
        }
    }
}

/// A multi-part archive set [`detect`] recognized around one member file.
///
/// `first_part` is the member an extraction (and therefore a merge) must
/// start from, derived from the convention rather than from whichever
/// member was passed in: `part1.rar` for [`MultiPartFormat::RarPart`],
/// the plain `.rar`/`.zip` for the sequence conventions, `.7z.001`/`.001`
/// for the numbered ones. It is not guaranteed to exist -- a set that
/// cannot be read from its start is exactly the "incomplete set" case
/// [`Self::parts`] reports as empty. Note that `first_part` is the entry
/// point, *not* necessarily `parts[0]` -- see [`Self::parts`].
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MultiPartArchiveDto {
    /// The set member a merge or extraction must start from. Carries a
    /// **lowercased** file name regardless of the real on-disk casing,
    /// because detection matches case-insensitively by lowercasing the
    /// whole name first -- a pre-existing `arclain_core` behavior this
    /// mirrors unchanged, and one that depends on a case-insensitive
    /// filesystem to resolve back to the real file.
    pub first_part: PathBuf,
    /// The set's name with the part indicator and extension stripped,
    /// lowercased for the same reason `first_part` is. The merged
    /// output's default file name is built from this.
    pub base_name: String,
    pub format: MultiPartFormat,
    /// Every member of the set found on disk, in the order a merge reads
    /// them. Enumeration stops at the first gap, so a set missing an
    /// early member reports fewer parts than exist on disk -- and an
    /// empty list means the set is unreadable from its start, which
    /// [`crate::ArclainApp::start_merge`] rejects rather than attempting.
    ///
    /// **`parts[0]` is not always `first_part`.** For
    /// [`MultiPartFormat::ZipSplit`] the enumeration is `.z01, .z02, …`
    /// followed by the `.zip` *last*, while `first_part` is the `.zip` --
    /// so for that one convention `parts.last() == first_part`. That is
    /// `arclain_core`'s own ordering (a split ZIP's central directory
    /// lives in the final `.zip`, which is what an extractor is pointed
    /// at), mirrored unchanged. Address the entry point through
    /// `first_part`, never through `parts[0]`.
    pub parts: Vec<PathBuf>,
}

/// Reports whether `path` is a member of a multi-part archive set, and
/// if so describes the whole set.
///
/// Wraps `arclain_core::archive::MultiPartArchive::detect` plus its
/// `find_all_parts` enumeration, so the returned value describes a set
/// that actually exists rather than only the naming pattern that matched.
///
/// # Filesystem access
///
/// Detection is mostly name-shaped, but not entirely -- three kinds of
/// filesystem probe happen, all of them `Path::exists` on a sibling file:
///
/// 1. A bare `name.rar` only counts as a sequence member if `name.r00`
///    exists beside it (otherwise every ordinary RAR would look
///    multi-part).
/// 2. A bare `name.zip` likewise only counts if `name.z01` exists.
/// 3. Once a convention matched, every member is probed in order to fill
///    [`MultiPartArchiveDto::parts`], stopping at the first gap.
///
/// Nothing is opened, read, or written. `None` means "not part of a
/// set", which is an ordinary answer rather than an error -- there is no
/// input for which detection itself fails.
pub fn detect_multipart(path: &Path) -> Option<MultiPartArchiveDto> {
    let mut detected = arclain_core::archive::MultiPartArchive::detect(path)?;
    // `detect` leaves `all_parts` empty by contract; `find_all_parts` is
    // what populates it. Its only failure mode is a `first_part` with no
    // parent directory at all, which cannot happen here (`detect` itself
    // already required `path.parent()` to build `first_part`), so an
    // error is folded into "no parts found" rather than propagated as a
    // detection failure.
    let parts = detected
        .find_all_parts()
        .map(<[PathBuf]>::to_vec)
        .unwrap_or_default();
    Some(MultiPartArchiveDto {
        first_part: detected.first_part,
        base_name: detected.base_name,
        format: MultiPartFormat::from_core(detected.format),
        parts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates `dir/name` as an empty file so an existence probe finds it.
    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), b"").expect("write fixture part");
    }

    #[test]
    fn every_format_description_matches_cores_own_text() {
        for (dto, core) in [
            (
                MultiPartFormat::RarPart,
                arclain_core::archive::MultiPartFormat::RarPart,
            ),
            (
                MultiPartFormat::RarSequence,
                arclain_core::archive::MultiPartFormat::RarSequence,
            ),
            (
                MultiPartFormat::SevenZip,
                arclain_core::archive::MultiPartFormat::SevenZip,
            ),
            (
                MultiPartFormat::ZipSplit,
                arclain_core::archive::MultiPartFormat::ZipSplit,
            ),
            (
                MultiPartFormat::Generic001,
                arclain_core::archive::MultiPartFormat::Generic001,
            ),
        ] {
            assert_eq!(dto.description(), core.description());
            assert_eq!(MultiPartFormat::from_core(core), dto);
            assert_eq!(dto.to_core(), core);
        }
    }

    #[test]
    fn format_serializes_snake_case_and_round_trips() {
        for (format, expected) in [
            (MultiPartFormat::RarPart, "rar_part"),
            (MultiPartFormat::RarSequence, "rar_sequence"),
            (MultiPartFormat::SevenZip, "seven_zip"),
            (MultiPartFormat::ZipSplit, "zip_split"),
            (MultiPartFormat::Generic001, "generic001"),
        ] {
            let value = serde_json::to_value(format).expect("serialize format");
            assert_eq!(value, serde_json::json!(expected));
            let round_tripped: MultiPartFormat =
                serde_json::from_value(value).expect("deserialize format");
            assert_eq!(round_tripped, format);
        }
    }

    /// The naming-convention table, checked against `arclain_core`'s own
    /// `detect` for every case so the facade can never recognize a
    /// different set of names than core does. Product codes are
    /// placeholders, not real catalogue entries.
    #[test]
    fn detection_agrees_with_core_across_the_naming_table() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let dir = temp.path();

        // A .partN.rar set, entered from a middle member.
        touch(dir, "RJ123456.part1.rar");
        touch(dir, "RJ123456.part2.rar");
        // A .rar/.r00 sequence.
        touch(dir, "RJ222222.rar");
        touch(dir, "RJ222222.r00");
        // A .7z.NNN split.
        touch(dir, "RJ333333.7z.001");
        touch(dir, "RJ333333.7z.002");
        // A .zNN/.zip split.
        touch(dir, "RJ444444.zip");
        touch(dir, "RJ444444.z01");
        // A generic .NNN split.
        touch(dir, "RJ555555.001");
        // Case-mixed .partN.rar.
        touch(dir, "RJ666666.Part1.RAR");
        // Non-members: an ordinary archive of each single-file kind.
        touch(dir, "RJ777777.zip");
        touch(dir, "RJ888888.rar");
        touch(dir, "RJ999999.7z");
        touch(dir, "notes.txt");

        let expected: &[(&str, Option<(MultiPartFormat, &str)>)] = &[
            (
                "RJ123456.part2.rar",
                Some((MultiPartFormat::RarPart, "rj123456")),
            ),
            (
                "RJ123456.part1.rar",
                Some((MultiPartFormat::RarPart, "rj123456")),
            ),
            (
                "RJ222222.rar",
                Some((MultiPartFormat::RarSequence, "rj222222")),
            ),
            (
                "RJ222222.r00",
                Some((MultiPartFormat::RarSequence, "rj222222")),
            ),
            (
                "RJ333333.7z.002",
                Some((MultiPartFormat::SevenZip, "rj333333")),
            ),
            (
                "RJ444444.zip",
                Some((MultiPartFormat::ZipSplit, "rj444444")),
            ),
            (
                "RJ444444.z01",
                Some((MultiPartFormat::ZipSplit, "rj444444")),
            ),
            (
                "RJ555555.001",
                Some((MultiPartFormat::Generic001, "rj555555")),
            ),
            (
                "RJ666666.Part1.RAR",
                Some((MultiPartFormat::RarPart, "rj666666")),
            ),
            // A lone .zip with no .z01 sibling is an ordinary archive.
            ("RJ777777.zip", None),
            // Same for a lone .rar with no .r00 sibling.
            ("RJ888888.rar", None),
            ("RJ999999.7z", None),
            ("notes.txt", None),
        ];

        for (file_name, want) in expected {
            let path = dir.join(file_name);
            let core = arclain_core::archive::MultiPartArchive::detect(&path);
            let dto = detect_multipart(&path);
            assert_eq!(
                dto.is_some(),
                core.is_some(),
                "{file_name}: facade and core must agree on whether this is a multi-part member"
            );
            match (want, dto) {
                (Some((format, base_name)), Some(dto)) => {
                    assert_eq!(dto.format, *format, "{file_name}: format");
                    assert_eq!(dto.base_name, *base_name, "{file_name}: base name");
                    let core = core.expect("core agreed this is a member");
                    assert_eq!(dto.first_part, core.first_part, "{file_name}: first part");
                    assert_eq!(
                        dto.format,
                        MultiPartFormat::from_core(core.format),
                        "{file_name}: format must mirror core's own"
                    );
                    assert_eq!(dto.base_name, core.base_name, "{file_name}: base name");
                }
                (None, None) => {}
                (want, dto) => panic!("{file_name}: expected {want:?}, detected {dto:?}"),
            }
        }
    }

    /// `detect` alone leaves the part list empty (core populates it in a
    /// separate `find_all_parts` step); the facade's DTO reports the
    /// parts that actually exist, in order.
    #[test]
    fn parts_are_enumerated_in_order_unlike_cores_bare_detect() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let dir = temp.path();
        touch(dir, "rj123456.part1.rar");
        touch(dir, "rj123456.part2.rar");
        touch(dir, "rj123456.part3.rar");

        let entered_from = dir.join("rj123456.part3.rar");
        let bare = arclain_core::archive::MultiPartArchive::detect(&entered_from)
            .expect("core must detect this member");
        assert!(
            bare.all_parts.is_empty(),
            "core's bare detect never enumerates parts"
        );

        let dto = detect_multipart(&entered_from).expect("facade must detect this member");
        assert_eq!(
            dto.parts,
            vec![
                dir.join("rj123456.part1.rar"),
                dir.join("rj123456.part2.rar"),
                dir.join("rj123456.part3.rar"),
            ]
        );
        assert_eq!(dto.first_part, dir.join("rj123456.part1.rar"));
    }

    /// A split ZIP's enumeration ends, rather than begins, with its entry
    /// point: `find_zip_split_files` collects `.z01, .z02, …` and appends
    /// the `.zip` last, while `first_part` is the `.zip`. Pinned because
    /// the DTO documents exactly this exception, and because a consumer
    /// reaching for `parts[0]` as "the first part" would be wrong here
    /// and only here.
    #[test]
    fn a_split_zips_entry_point_is_its_last_enumerated_part() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let dir = temp.path();
        touch(dir, "rj444444.z01");
        touch(dir, "rj444444.z02");
        touch(dir, "rj444444.zip");

        let dto = detect_multipart(&dir.join("rj444444.z01")).expect("member is detected");
        assert_eq!(dto.format, MultiPartFormat::ZipSplit);
        assert_eq!(
            dto.parts,
            vec![
                dir.join("rj444444.z01"),
                dir.join("rj444444.z02"),
                dir.join("rj444444.zip"),
            ]
        );
        assert_eq!(dto.first_part, dir.join("rj444444.zip"));
        assert_eq!(
            dto.parts.last(),
            Some(&dto.first_part),
            "for a split ZIP the entry point is the last enumerated part"
        );
        assert_ne!(
            dto.parts.first(),
            Some(&dto.first_part),
            "...and specifically not the first, which is the exception the DTO documents"
        );
    }

    /// Writes `probe` and checks whether `PROBE` resolves to it, i.e.
    /// whether this filesystem is case-insensitive. Probed at runtime
    /// rather than keyed off `cfg!(windows)`: a case-sensitive volume on
    /// Windows and a case-insensitive one on macOS both exist, and the
    /// behaviour under test depends on the volume, not the OS.
    fn filesystem_is_case_insensitive(dir: &Path) -> bool {
        let lower = dir.join("case-probe");
        std::fs::write(&lower, b"").expect("write case probe");
        let resolved = dir.join("CASE-PROBE").exists();
        std::fs::remove_file(&lower).expect("remove case probe");
        resolved
    }

    /// Pins a pre-existing `arclain_core` behaviour the facade passes
    /// through unchanged, **and its platform consequence**: detection
    /// lowercases the whole file name before matching, so every path it
    /// reports (`first_part`, and therefore every enumerated part) is
    /// lowercased whatever the real on-disk casing was.
    ///
    /// That only round-trips back to the real files on a case-insensitive
    /// filesystem. On a case-sensitive one, `rj123456.part1.rar` does not
    /// resolve to `RJ123456.Part1.RAR`, enumeration finds nothing, and the
    /// set is refused by `start_merge` as `NotFound`: **an
    /// uppercase-named split archive is unmergeable there.** Both
    /// outcomes are asserted rather than one of them being allowed to pass
    /// vacuously through an `all()` over an empty list, so this test
    /// states the limitation executably on every platform.
    #[test]
    fn reported_paths_carry_cores_lowercased_file_names() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let dir = temp.path();
        touch(dir, "RJ123456.Part1.RAR");
        touch(dir, "RJ123456.Part2.RAR");

        let dto = detect_multipart(&dir.join("RJ123456.Part2.RAR")).expect("member is detected");
        assert_eq!(dto.base_name, "rj123456");
        assert_eq!(dto.first_part, dir.join("rj123456.part1.rar"));
        assert!(
            dto.parts.iter().all(|part| {
                part.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name == name.to_lowercase())
            }),
            "every reported part path carries a lowercased file name: {:?}",
            dto.parts
        );

        if filesystem_is_case_insensitive(dir) {
            assert_eq!(
                dto.parts,
                vec![
                    dir.join("rj123456.part1.rar"),
                    dir.join("rj123456.part2.rar"),
                ],
                "a case-insensitive filesystem resolves the lowercased names, so both parts \
                 are found"
            );
        } else {
            assert!(
                dto.parts.is_empty(),
                "on a case-sensitive filesystem the lowercased names resolve to nothing, so an \
                 uppercase-named set enumerates no parts and start_merge refuses it: {:?}",
                dto.parts
            );
        }
    }

    /// Enumeration stops at the first gap, so a set entered from a member
    /// whose predecessors are missing reports no parts at all -- the
    /// signal `start_merge` refuses to proceed on.
    #[test]
    fn a_set_whose_first_part_is_missing_reports_no_parts() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let dir = temp.path();
        touch(dir, "RJ123456.part2.rar");
        touch(dir, "RJ123456.part3.rar");

        let dto =
            detect_multipart(&dir.join("RJ123456.part2.rar")).expect("the naming pattern matches");
        assert_eq!(dto.format, MultiPartFormat::RarPart);
        assert!(
            dto.parts.is_empty(),
            "enumeration starts at part1 and stops at the first gap"
        );
    }

    #[test]
    fn detection_is_idempotent_on_the_first_part_it_reports() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let dir = temp.path();
        for name in [
            "RJ123456.part1.rar",
            "RJ123456.part2.rar",
            "RJ222222.rar",
            "RJ222222.r00",
            "RJ333333.7z.001",
            "RJ444444.zip",
            "RJ444444.z01",
            "RJ555555.001",
        ] {
            touch(dir, name);
        }

        for entry in [
            "RJ123456.part2.rar",
            "RJ222222.r00",
            "RJ333333.7z.001",
            "RJ444444.z01",
            "RJ555555.001",
        ] {
            let first = detect_multipart(&dir.join(entry)).expect("member must be detected");
            let again = detect_multipart(&first.first_part)
                .expect("re-detecting a set's own first part must succeed");
            assert_eq!(
                (again.format, &again.base_name, &again.first_part),
                (first.format, &first.base_name, &first.first_part),
                "{entry}: re-detection on first_part must reproduce the same identity"
            );
        }
    }

    #[test]
    fn dto_serializes_and_round_trips() {
        let dto = MultiPartArchiveDto {
            first_part: PathBuf::from("/sets/RJ123456.part1.rar"),
            base_name: "rj123456".to_string(),
            format: MultiPartFormat::RarPart,
            parts: vec![PathBuf::from("/sets/RJ123456.part1.rar")],
        };
        let value = serde_json::to_value(&dto).expect("serialize dto");
        let round_tripped: MultiPartArchiveDto =
            serde_json::from_value(value).expect("deserialize dto");
        assert_eq!(round_tripped, dto);
    }
}
