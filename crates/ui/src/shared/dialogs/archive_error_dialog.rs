//! Modal shown when an archive backend operation fails for a non-password
//! reason. Classifies the failure and, when it's a recognizable case
//! (currently EACCES / Permission denied), shows the exact terminal
//! commands needed to fix it — templated against the failing path so the
//! user can copy-paste without editing placeholders. For permission
//! errors, also surfaces the concrete reason (owner uid/name + mode bits
//! + current uid) so the dialog answers the "why" not just the "what".

use super::helpers::{show_dimmed_modal, ModalParams};
use crate::shared::theme::AppTheme;
use arclain_theme::ButtonVariant;
use arclain_widgets::{ButtonSize, TextButton};
use eframe::egui;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArchiveErrorDialogState {
    pub show: bool,
    pub archive_path: Option<PathBuf>,
    pub kind: ArchiveErrorKind,
    pub raw_error: String,
    /// Filled in at error time for permission errors so we can show the
    /// concrete owner/mode/current-uid in the dialog without doing an
    /// extra `stat()` on every frame.
    pub diagnostic: Option<FileDiagnostic>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum ArchiveErrorKind {
    /// Unknown / unclassified backend failure. We surface the raw error.
    #[default]
    Generic,
    /// EACCES — the running user can't read the file. Has the most
    /// elaborate dialog branch: shows owner/mode/current-uid plus the
    /// exact chown/chmod commands to fix it.
    PermissionDenied,
    /// ENOENT — file doesn't exist (anymore). Common when a path lives
    /// on a tmpfs/cache that got cleaned, or an NFS export got
    /// unmounted between drop and open.
    FileNotFound,
    /// EISDIR — the path is a directory, not a file. Usually means the
    /// user dragged a folder onto arclain by mistake.
    IsADirectory,
    /// EIO / ENOSPC / disk-layer trouble. Reading the file's bytes
    /// fails at the kernel-block level, not at the open() syscall.
    IoError,
    /// Backend opened the file but can't parse it as the expected
    /// archive format — wrong magic bytes, truncated header, or the
    /// extension lies about what's inside.
    CorruptedOrWrongFormat,
}

/// Concrete file ownership + mode + running-user info captured at
/// error time. Display-ready: name fields are `None` only when the
/// uid/gid couldn't be resolved (no matching passwd/group entry —
/// happens for orphan UIDs from removed accounts or container
/// userns mappings).
#[derive(Debug, Clone, PartialEq)]
pub struct FileDiagnostic {
    pub owner_uid: u32,
    pub owner_name: Option<String>,
    pub group_gid: u32,
    pub group_name: Option<String>,
    /// Raw permission bits (e.g. `0o600`).
    pub mode_bits: u32,
    /// `ls -l`-style permission string (e.g. `-rw-------`).
    pub mode_string: String,
    pub current_uid: u32,
    pub current_user: Option<String>,
}

/// Classify a backend error message into one of the known reasons-
/// arclain-can't-open-this-file. Mirrors the `is_password_error()`
/// pattern in `core/operations/archive.rs` — string-match on tokens
/// the backends emit. Order matters: EACCES is the most common single
/// reason so we check it first; format/parse failures are the catch-
/// all bucket so they're checked last.
///
/// Patterns are drawn from:
///   - 7-Zip CLI:       `errno=13 : Permission denied`, `errno=2 : No such file`
///   - UnRAR CLI:       `Permission denied`, `Cannot find the file`
///   - native sevenz_rust2 / unrar crate: `Failed to open`
///   - Rust std::io:    `os error N` (kernel errno) + the English message
///
/// Add a new arm any time a backend surfaces an error that lands in
/// the `Generic` bucket but warrants its own user-friendly story.
pub fn classify(err_msg: &str) -> ArchiveErrorKind {
    let m = err_msg;
    // ── EACCES — the headline case ────────────────────────────────────
    if m.contains("Permission denied")
        || m.contains("EACCES")
        || has_errno(m, 13)
        || has_os_error(m, 13)
    {
        return ArchiveErrorKind::PermissionDenied;
    }
    // ── EISDIR — user dropped a directory ────────────────────────────
    // Checked BEFORE FileNotFound: errno=21 / "os error 21" must not
    // fall through to the substring match for errno=2 / "os error 2"
    // (the regression that bit us before the helper functions landed).
    if m.contains("Is a directory")
        || m.contains("EISDIR")
        || has_errno(m, 21)
        || has_os_error(m, 21)
    {
        return ArchiveErrorKind::IsADirectory;
    }
    // ── EIO / ENOSPC — disk-layer trouble ────────────────────────────
    // Also checked before FileNotFound for the same substring-bleed
    // reason (errno=28 contains errno=2).
    if m.contains("Input/output error")
        || m.contains("EIO")
        || m.contains("No space left")
        || m.contains("ENOSPC")
        || has_errno(m, 5)
        || has_os_error(m, 5)
        || has_errno(m, 28)
        || has_os_error(m, 28)
    {
        return ArchiveErrorKind::IoError;
    }
    // ── ENOENT — file is gone ────────────────────────────────────────
    if m.contains("No such file")
        || m.contains("Cannot find the file")
        || m.contains("ENOENT")
        || has_errno(m, 2)
        || has_os_error(m, 2)
    {
        return ArchiveErrorKind::FileNotFound;
    }
    // ── Format / parse failures ──────────────────────────────────────
    // Match what the native sevenz_rust2 + unrar crates emit when
    // headers don't parse, and what 7z/unrar CLI say for "this isn't
    // the format I expected".
    if m.contains("Failed to open 7z archive")
        || m.contains("not an archive")
        || m.contains("Unknown header")
        || m.contains("Bad archive")
        || m.contains("corrupted")
        || m.contains("Corrupt")
        || m.contains("not a 7z")
        || m.contains("Wrong password")  // sometimes leaks through here
    {
        // Note: pure "Wrong password" should be intercepted by
        // is_password_error() upstream and never reach this classifier,
        // but matching it here as well keeps the worst-case fallback
        // sane (better "corrupted or wrong format" than "Generic").
        return ArchiveErrorKind::CorruptedOrWrongFormat;
    }
    ArchiveErrorKind::Generic
}

// Word-boundary-aware errno match: returns true iff `s` contains
// "errno=<N>" followed by EOF or a non-digit. Plain substring match
// can't be used because "errno=2" is a substring of "errno=21" — and
// EISDIR (21) being misclassified as ENOENT (2) is exactly the
// regression these helpers prevent.
fn has_errno(s: &str, code: u32) -> bool {
    has_with_trailing_nondigit(s, &format!("errno={code}"))
}

fn has_os_error(s: &str, code: u32) -> bool {
    has_with_trailing_nondigit(s, &format!("os error {code}"))
}

fn has_with_trailing_nondigit(haystack: &str, needle: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        let abs = start + pos;
        let after = abs + needle.len();
        match bytes.get(after) {
            None => return true,
            Some(b) if !b.is_ascii_digit() => return true,
            _ => start = abs + 1, // shift past this hit, keep searching
        }
    }
    false
}

/// Capture concrete file ownership + mode + running-user identity for a
/// path that just failed to open. Returns `None` if the file no longer
/// exists or we can't stat it — the dialog falls back to a generic
/// "permission denied" body in that case.
///
/// Called from the load-failure path in `core/operations/archive.rs`
/// when the error classifies as `PermissionDenied`. The returned data
/// goes straight into `ArchiveErrorDialogState.diagnostic` and gets
/// rendered verbatim — no extra syscalls on subsequent frames.
#[cfg(unix)]
pub fn gather_diagnostic(path: &Path) -> Option<FileDiagnostic> {
    use std::os::unix::fs::MetadataExt;

    let meta = std::fs::symlink_metadata(path).ok()?;
    let owner_uid = meta.uid();
    let group_gid = meta.gid();
    let mode = meta.mode();
    // mode() returns the full st_mode (type + perms). The dialog only
    // cares about the perm bits for the "-rwxr-x---" string; the file
    // type (regular file vs directory vs …) doesn't matter here.
    let mode_string = format_mode(mode);

    // SAFETY: getuid() is async-signal-safe and never fails.
    let current_uid = unsafe { libc::getuid() } as u32;

    Some(FileDiagnostic {
        owner_uid,
        owner_name: lookup_user_name(owner_uid),
        group_gid,
        group_name: lookup_group_name(group_gid),
        mode_bits: mode,
        mode_string,
        current_uid,
        current_user: lookup_user_name(current_uid),
    })
}

#[cfg(not(unix))]
pub fn gather_diagnostic(_path: &Path) -> Option<FileDiagnostic> {
    // Non-Unix (Windows): permission semantics are ACL-based, not
    // POSIX-style, and the EACCES classifier path doesn't fire for
    // ACL denials anyway. Returning None keeps the dialog rendering
    // a generic "permission denied" message without empty diagnostic
    // fields. If/when arclain ships a Windows-permission diagnostic
    // path, swap this out with one that walks the DACL.
    None
}

/// Resolve a uid to its `passwd` name via `getpwuid_r`. Returns `None`
/// when no `passwd` entry matches (orphan uids from deleted accounts
/// or container userns mappings that don't have an /etc/passwd entry
/// on the host).
#[cfg(unix)]
fn lookup_user_name(uid: u32) -> Option<String> {
    // Buffer sized per `sysconf(_SC_GETPW_R_SIZE_MAX)` recommendation
    // with a safe fallback. Most systems return 1024-16384.
    let mut buf = vec![0u8; 4096];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();

    // SAFETY: We pass valid pointers to a stack-allocated `passwd`,
    // a heap-allocated buffer of known size, and a `result` out-param.
    // Per POSIX, on success `result` points to `pwd`; on no-match it
    // stays null. The function does not retain any of the pointers
    // after return.
    let rc = unsafe {
        libc::getpwuid_r(
            uid as libc::uid_t,
            &mut pwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };

    if rc != 0 || result.is_null() {
        return None;
    }
    // SAFETY: pwd.pw_name is valid for the lifetime of buf — both live
    // until this function returns. We copy out as an owned String before
    // either is dropped.
    let name_cstr = unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) };
    Some(name_cstr.to_string_lossy().into_owned())
}

#[cfg(unix)]
fn lookup_group_name(gid: u32) -> Option<String> {
    let mut buf = vec![0u8; 4096];
    let mut grp: libc::group = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::group = std::ptr::null_mut();

    // SAFETY: same pattern as `lookup_user_name`.
    let rc = unsafe {
        libc::getgrgid_r(
            gid as libc::gid_t,
            &mut grp,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return None;
    }
    let name_cstr = unsafe { std::ffi::CStr::from_ptr(grp.gr_name) };
    Some(name_cstr.to_string_lossy().into_owned())
}

/// Format a POSIX mode word the way `ls -l` does — `-rwxr-x---` etc.
/// Includes the file-type character. Doesn't handle setuid/setgid/sticky
/// (rarely relevant for archive files and would clutter the dialog).
///
/// `#[cfg(unix)]` to match its only caller, the unix `gather_diagnostic`
/// — POSIX mode bits are meaningless on Windows, where the dialog uses
/// the `#[cfg(not(unix))]` stub that never gathers ownership/mode. Kept
/// out of the Windows build entirely (otherwise it's dead code there).
#[cfg(unix)]
fn format_mode(mode: u32) -> String {
    let file_type = match mode & 0o170000 {
        0o040000 => 'd',
        0o120000 => 'l',
        0o100000 => '-',
        0o060000 => 'b',
        0o020000 => 'c',
        0o010000 => 'p',
        0o140000 => 's',
        _ => '?',
    };
    let mut out = String::with_capacity(10);
    out.push(file_type);
    let perm = mode & 0o777;
    let triplet = |bits: u32| {
        let mut s = String::with_capacity(3);
        s.push(if bits & 0o4 != 0 { 'r' } else { '-' });
        s.push(if bits & 0o2 != 0 { 'w' } else { '-' });
        s.push(if bits & 0o1 != 0 { 'x' } else { '-' });
        s
    };
    out.push_str(&triplet((perm >> 6) & 0o7));
    out.push_str(&triplet((perm >> 3) & 0o7));
    out.push_str(&triplet(perm & 0o7));
    out
}

pub fn render_archive_error_dialog(
    ctx: &egui::Context,
    theme: &AppTheme,
    state: &mut ArchiveErrorDialogState,
) {
    if !state.show {
        return;
    }

    let params = ModalParams {
        width_frac: 0.5,
        height_frac: 0.65,
        min: egui::vec2(580.0, 400.0),
        max: egui::vec2(780.0, 640.0),
        bottom_bar_height: 48.0,
        ..Default::default()
    };

    let archive_path = state.archive_path.clone();
    let kind = state.kind.clone();
    let raw_error = state.raw_error.clone();
    let diagnostic = state.diagnostic.clone();

    show_dimmed_modal(
        ctx,
        theme,
        "archive_error",
        &params,
        |ui, _rect| {
            let title = match kind {
                ArchiveErrorKind::PermissionDenied => "Can't open archive — permission denied",
                ArchiveErrorKind::FileNotFound => "Can't open archive — file not found",
                ArchiveErrorKind::IsADirectory => "Can't open archive — that's a folder",
                ArchiveErrorKind::IoError => "Can't open archive — disk error",
                ArchiveErrorKind::CorruptedOrWrongFormat => {
                    "Can't open archive — corrupted or unsupported format"
                }
                ArchiveErrorKind::Generic => "Can't open archive",
            };
            ui.label(
                egui::RichText::new(title)
                    .size(18.0)
                    .color(theme.colors.error)
                    .strong(),
            );
            ui.add_space(8.0);

            if let Some(path) = &archive_path {
                render_path_block(ui, theme, &path.display().to_string());
            }
            ui.add_space(12.0);

            match kind {
                ArchiveErrorKind::PermissionDenied => render_permission_section(
                    ui,
                    theme,
                    archive_path.as_deref(),
                    diagnostic.as_ref(),
                ),
                ArchiveErrorKind::FileNotFound => render_not_found_section(ui, theme),
                ArchiveErrorKind::IsADirectory => render_is_directory_section(ui, theme),
                ArchiveErrorKind::IoError => {
                    render_io_error_section(ui, theme, &raw_error)
                }
                ArchiveErrorKind::CorruptedOrWrongFormat => {
                    render_format_error_section(ui, theme, archive_path.as_deref(), &raw_error)
                }
                ArchiveErrorKind::Generic => render_generic_section(ui, theme, &raw_error),
            }
        },
        |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        TextButton::new("Close", ButtonSize::Medium)
                            .variant(ButtonVariant::Primary),
                    )
                    .clicked()
                {
                    state.show = false;
                }
            });
        },
    );
}

fn render_permission_section(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    path: Option<&Path>,
    diagnostic: Option<&FileDiagnostic>,
) {
    // ── WHY (concrete) ────────────────────────────────────────────────
    // Spell out the actual ownership + mode so the user doesn't have
    // to `ls -l` to know what's blocking the read.
    ui.label(
        egui::RichText::new("Reason")
            .color(theme.colors.on_surface)
            .strong(),
    );
    ui.add_space(4.0);

    if let Some(d) = diagnostic {
        let owner_label = d
            .owner_name
            .as_deref()
            .map(|n| format!("{n} (uid {})", d.owner_uid))
            .unwrap_or_else(|| format!("uid {}", d.owner_uid));
        let group_label = d
            .group_name
            .as_deref()
            .map(|n| format!("{n} (gid {})", d.group_gid))
            .unwrap_or_else(|| format!("gid {}", d.group_gid));
        let me_label = d
            .current_user
            .as_deref()
            .map(|n| format!("{n} (uid {})", d.current_uid))
            .unwrap_or_else(|| format!("uid {}", d.current_uid));

        let reason = format!(
            "The file is owned by {} : {} with mode {} ({:#o}).\n\
             You are {}, which is neither the owner nor in the group, \
             and the mode doesn't grant read access to other users.",
            owner_label,
            group_label,
            d.mode_string,
            d.mode_bits & 0o777,
            me_label,
        );
        ui.label(egui::RichText::new(reason).color(theme.colors.on_surface_variant));

        if d.owner_uid == 65534 {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "uid 65534 ('nfsnobody' on Fedora) is the standard ID for files \
                     written by NFS exports with root-squash, or by rootless container \
                     processes whose userns mapping fell off the end. Both are common.",
                )
                .color(theme.colors.on_surface_variant)
                .italics()
                .small(),
            );
        }
    } else {
        // Couldn't stat — fall back to a generic explanation.
        ui.label(
            egui::RichText::new(
                "The file exists but the current user can't read it. \
                 This usually happens when the archive was created by \
                 another user, written by a container with userns \
                 mapping, or restored from an NFS export.",
            )
            .color(theme.colors.on_surface_variant),
        );
    }
    ui.add_space(12.0);

    // ── FIX (deterministic commands) ──────────────────────────────────
    ui.label(
        egui::RichText::new("Fix — run one of these in a terminal")
            .color(theme.colors.on_surface)
            .strong(),
    );
    ui.add_space(6.0);

    let (chown_cmd, chmod_cmd, chown_dir_cmd) = if let Some(p) = path {
        let quoted = shell_quote(&p.display().to_string());
        let parent_cmd = p.parent().map(|d| {
            format!(
                "sudo chown -R \"$USER\":\"$USER\" {}",
                shell_quote(&d.display().to_string())
            )
        });
        (
            format!("sudo chown \"$USER\":\"$USER\" {}", quoted),
            format!("sudo chmod u+r {}", quoted),
            parent_cmd,
        )
    } else {
        (
            "sudo chown \"$USER\":\"$USER\" <file>".to_string(),
            "sudo chmod u+r <file>".to_string(),
            None,
        )
    };

    render_command(ui, theme, &chown_cmd, "Take ownership of this file");
    render_command(ui, theme, &chmod_cmd, "Or grant yourself read permission");
    if let Some(dir_cmd) = chown_dir_cmd.as_deref() {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("If many files in the same directory have the same issue:")
                .color(theme.colors.on_surface_variant)
                .small(),
        );
        ui.add_space(4.0);
        render_command(ui, theme, dir_cmd, "Take ownership of the whole directory");
    }
}

fn render_not_found_section(ui: &mut egui::Ui, theme: &AppTheme) {
    ui.label(
        egui::RichText::new("Reason")
            .color(theme.colors.on_surface)
            .strong(),
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "The file doesn't exist at that path anymore. The path may \
             have lived on a tmpfs that got cleaned (`/tmp`, `/run`), an \
             NFS / SMB / sshfs mount that was unmounted, or a sandboxed \
             filesystem that the host doesn't have access to.",
        )
        .color(theme.colors.on_surface_variant),
    );
    ui.add_space(12.0);
    ui.label(
        egui::RichText::new("Fix")
            .color(theme.colors.on_surface)
            .strong(),
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "Re-locate the file in your file manager (Dolphin, Nautilus, …) \
             and drag it in again. If the file lives on a removable mount, \
             check the mount is still attached.",
        )
        .color(theme.colors.on_surface_variant),
    );
}

fn render_is_directory_section(ui: &mut egui::Ui, theme: &AppTheme) {
    ui.label(
        egui::RichText::new("Reason")
            .color(theme.colors.on_surface)
            .strong(),
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "That path is a folder, not an archive file. Arclain reads \
             archive files (`.zip`, `.7z`, `.rar`, `.tar.*`); it doesn't \
             treat folder trees as archives.",
        )
        .color(theme.colors.on_surface_variant),
    );
    ui.add_space(12.0);
    ui.label(
        egui::RichText::new("Fix")
            .color(theme.colors.on_surface)
            .strong(),
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "Drop a single archive file instead. If you wanted to compress \
             a folder, create an archive from it first using your file \
             manager's right-click menu or `7z a archive.7z folder/`.",
        )
        .color(theme.colors.on_surface_variant),
    );
}

fn render_io_error_section(ui: &mut egui::Ui, theme: &AppTheme, raw_error: &str) {
    ui.label(
        egui::RichText::new("Reason")
            .color(theme.colors.on_surface)
            .strong(),
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "The kernel couldn't read the file's bytes — this is a disk- \
             or filesystem-layer failure, not a permission or format \
             problem. Causes range from a dying drive (bad sectors, SMART \
             errors), a flaky USB / network mount, a stuck filesystem \
             driver, or out-of-space conditions on the volume.",
        )
        .color(theme.colors.on_surface_variant),
    );
    ui.add_space(12.0);
    ui.label(
        egui::RichText::new("Fix")
            .color(theme.colors.on_surface)
            .strong(),
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "Check `dmesg` for kernel I/O errors. If the file lives on a \
             USB drive or network share, remount and try again. For an \
             internal disk surfacing repeated I/O errors, copy any \
             reachable archives off the drive ASAP.",
        )
        .color(theme.colors.on_surface_variant),
    );
    ui.add_space(12.0);
    render_raw_error_block(ui, theme, raw_error);
}

fn render_format_error_section(
    ui: &mut egui::Ui,
    theme: &AppTheme,
    path: Option<&Path>,
    raw_error: &str,
) {
    ui.label(
        egui::RichText::new("Reason")
            .color(theme.colors.on_surface)
            .strong(),
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "The file opened, but the archive backend didn't recognize its \
             contents as the format the extension claims. The file may be \
             truncated (interrupted download / copy), corrupted (failing \
             storage, bit rot), or simply mis-named (a `.zip` that's \
             actually a 7z, or a renamed text file).",
        )
        .color(theme.colors.on_surface_variant),
    );
    ui.add_space(12.0);
    ui.label(
        egui::RichText::new("Fix")
            .color(theme.colors.on_surface)
            .strong(),
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "Check what the file actually is. The `file` command identifies \
             the real format from the magic bytes regardless of the \
             extension:",
        )
        .color(theme.colors.on_surface_variant),
    );
    ui.add_space(6.0);
    let file_cmd = if let Some(p) = path {
        format!("file {}", shell_quote(&p.display().to_string()))
    } else {
        "file <path>".to_string()
    };
    render_command(ui, theme, &file_cmd, "Identify the real format from magic bytes");
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(
            "If `file` reports the wrong type vs the extension, rename to \
             match the real type and re-open. If `file` says the type matches \
             but arclain still rejects it, the archive is likely truncated \
             — re-download or re-copy from the original.",
        )
        .color(theme.colors.on_surface_variant),
    );
    ui.add_space(12.0);
    render_raw_error_block(ui, theme, raw_error);
}

fn render_raw_error_block(ui: &mut egui::Ui, theme: &AppTheme, raw_error: &str) {
    ui.label(
        egui::RichText::new("Raw error from backend")
            .color(theme.colors.on_surface_variant)
            .small(),
    );
    ui.add_space(4.0);
    egui::Frame::new()
        .fill(theme.colors.surface_variant)
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            // Wrap + selectable: backend errors are dumped verbatim
            // (often single long lines from 7z/unrar stderr), so without
            // wrap they overflow the modal. selectable() lets users
            // mouse-grab a substring (e.g. just the errno) for paste.
            ui.add(
                egui::Label::new(
                    egui::RichText::new(raw_error)
                        .monospace()
                        .color(theme.colors.on_surface_variant),
                )
                .wrap()
                .selectable(true),
            );
        });
}

fn render_generic_section(ui: &mut egui::Ui, theme: &AppTheme, raw_error: &str) {
    ui.label(
        egui::RichText::new("Reason")
            .color(theme.colors.on_surface)
            .strong(),
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "The archive backend couldn't open this file. The raw error \
             from the backend is below.",
        )
        .color(theme.colors.on_surface_variant),
    );
    ui.add_space(12.0);

    egui::Frame::new()
        .fill(theme.colors.surface_variant)
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(raw_error)
                    .monospace()
                    .color(theme.colors.on_surface_variant),
            );
        });
}

fn render_command(ui: &mut egui::Ui, theme: &AppTheme, cmd: &str, label: &str) {
    // Description line above the box. The box itself holds the command
    // (wrapped to box width) on the left and the Copy button on the
    // right — a `horizontal` row gives the button right-aligned
    // anchoring without it competing with the command for width.
    ui.label(
        egui::RichText::new(label)
            .color(theme.colors.on_surface_variant)
            .small(),
    );

    egui::Frame::new()
        .fill(theme.colors.surface_variant)
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            // Reserve right-edge space for the Copy button first so the
            // command label gets the remaining width to wrap into.
            // Without this, a horizontal layout lets the label run off
            // the right of the box because egui horizontals don't wrap.
            const COPY_BTN_WIDTH: f32 = 56.0;
            const COPY_BTN_GAP: f32 = 8.0;
            let avail = ui.available_width();
            let cmd_width = (avail - COPY_BTN_WIDTH - COPY_BTN_GAP).max(80.0);

            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(cmd_width, 0.0),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(cmd)
                                    .monospace()
                                    .color(theme.colors.on_surface),
                            )
                            // Wrap at the allocated box width so long
                            // shell-quoted paths stay inside the card
                            // and break on whitespace. Selectable so
                            // users can also copy a substring (e.g.
                            // just the path) with the mouse.
                            .wrap()
                            .selectable(true),
                        );
                    },
                );
                ui.add_space(COPY_BTN_GAP);
                if ui
                    .add(
                        TextButton::new("Copy", ButtonSize::Small)
                            .variant(ButtonVariant::Secondary),
                    )
                    .clicked()
                {
                    ui.ctx().copy_text(cmd.to_string());
                }
            });
        });
    ui.add_space(6.0);
}

// Same layout as `render_command` but tuned for the path header at the
// top of the dialog: a plain monospace label (no shell-quoting context)
// plus a Copy button so the user can grab the path for any tool of
// their choosing (terminal, file manager address bar, etc).
fn render_path_block(ui: &mut egui::Ui, theme: &AppTheme, path_str: &str) {
    egui::Frame::new()
        .fill(theme.colors.surface_variant)
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            const COPY_BTN_WIDTH: f32 = 56.0;
            const COPY_BTN_GAP: f32 = 8.0;
            let avail = ui.available_width();
            let label_width = (avail - COPY_BTN_WIDTH - COPY_BTN_GAP).max(80.0);

            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(label_width, 0.0),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(path_str)
                                    .monospace()
                                    .color(theme.colors.on_surface_variant),
                            )
                            .wrap()
                            .selectable(true),
                        );
                    },
                );
                ui.add_space(COPY_BTN_GAP);
                if ui
                    .add(
                        TextButton::new("Copy", ButtonSize::Small)
                            .variant(ButtonVariant::Secondary),
                    )
                    .clicked()
                {
                    ui.ctx().copy_text(path_str.to_string());
                }
            });
        });
}

// POSIX shell single-quote: needed so spaces/quotes in the archive path
// (e.g. "RAVEN SAVE.7z") survive copy-paste into the user's shell. Same
// approach as Python's shlex.quote.
fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c))
    {
        s.to_string()
    } else {
        let escaped = s.replace('\'', "'\\''");
        format!("'{}'", escaped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_permission_denied_from_7zip() {
        let err = "7-Zip command failed with code Some(2): ERROR: /run/media/system/foo.7z : opening : errno=13 : Permission denied";
        assert_eq!(classify(err), ArchiveErrorKind::PermissionDenied);
    }

    #[test]
    fn classify_permission_denied_from_io_error() {
        let err = "Permission denied (os error 13)";
        assert_eq!(classify(err), ArchiveErrorKind::PermissionDenied);
    }

    #[test]
    fn classify_generic_for_unknown() {
        assert_eq!(classify("Some other error"), ArchiveErrorKind::Generic);
    }

    #[test]
    fn classify_file_not_found() {
        assert_eq!(
            classify("No such file or directory (os error 2)"),
            ArchiveErrorKind::FileNotFound,
        );
        assert_eq!(
            classify("ERROR: archive.7z : opening : errno=2"),
            ArchiveErrorKind::FileNotFound,
        );
        assert_eq!(
            classify("UnRAR: Cannot find the file foo.rar"),
            ArchiveErrorKind::FileNotFound,
        );
    }

    #[test]
    fn classify_is_a_directory() {
        assert_eq!(
            classify("Is a directory (os error 21)"),
            ArchiveErrorKind::IsADirectory,
        );
    }

    #[test]
    fn classify_io_error() {
        assert_eq!(
            classify("Input/output error (os error 5)"),
            ArchiveErrorKind::IoError,
        );
        assert_eq!(
            classify("No space left on device (os error 28)"),
            ArchiveErrorKind::IoError,
        );
    }

    #[test]
    fn classify_corrupted_or_wrong_format() {
        assert_eq!(
            classify("Failed to open 7z archive: bad magic"),
            ArchiveErrorKind::CorruptedOrWrongFormat,
        );
        assert_eq!(
            classify("Archive is corrupted"),
            ArchiveErrorKind::CorruptedOrWrongFormat,
        );
        assert_eq!(
            classify("not a 7z file"),
            ArchiveErrorKind::CorruptedOrWrongFormat,
        );
    }

    #[test]
    fn classify_does_not_confuse_errno_substrings() {
        // The bug these helpers exist to prevent: "errno=21" / "os error 21"
        // contain "errno=2" / "os error 2" as substrings, so naive
        // substring matching would misclassify EISDIR as ENOENT. Same
        // story for errno=28 / ENOSPC.
        assert_eq!(
            classify("errno=21 : Is a directory"),
            ArchiveErrorKind::IsADirectory,
        );
        assert_eq!(
            classify("errno=28 : No space left on device"),
            ArchiveErrorKind::IoError,
        );
        assert_eq!(
            classify("os error 28"),
            ArchiveErrorKind::IoError,
        );
    }

    #[test]
    fn has_errno_helper_excludes_substring_match() {
        assert!(has_errno("got errno=2 something", 2));
        assert!(has_errno("got errno=2", 2));
        assert!(!has_errno("got errno=21", 2));
        assert!(!has_errno("got errno=20", 2));
        assert!(has_errno("got errno=20", 20));
    }

    #[test]
    fn classify_permission_takes_precedence_over_format() {
        // A backend can report both "permission denied" and "failed to
        // parse" in the same string (when EACCES happens during a parse
        // pass). Permission diagnosis is more actionable, so make sure
        // it wins the match-order race.
        let err = "Failed to open 7z archive: Permission denied";
        assert_eq!(classify(err), ArchiveErrorKind::PermissionDenied);
    }

    #[test]
    fn shell_quote_passes_simple() {
        assert_eq!(shell_quote("/home/user/file.7z"), "/home/user/file.7z");
    }

    #[test]
    fn shell_quote_wraps_spaces() {
        assert_eq!(
            shell_quote("/path with spaces/RAVEN SAVE.7z"),
            "'/path with spaces/RAVEN SAVE.7z'"
        );
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("don't.7z"), "'don'\\''t.7z'");
    }

    #[cfg(unix)]
    #[test]
    fn format_mode_renders_ls_minus_l_string() {
        // -rw------- (regular file, owner-read+write only)
        assert_eq!(format_mode(0o100600), "-rw-------");
        // -rw-r--r-- (standard "world-readable file")
        assert_eq!(format_mode(0o100644), "-rw-r--r--");
        // drwxr-xr-x (directory with standard perms)
        assert_eq!(format_mode(0o040755), "drwxr-xr-x");
    }

    #[cfg(unix)]
    #[test]
    fn gather_diagnostic_returns_some_for_existing_file() {
        // Use an always-present file. /etc/hostname is short, world-
        // readable, and exists on every Linux installation.
        let path = Path::new("/etc/hostname");
        if path.exists() {
            let d = gather_diagnostic(path).expect("should stat /etc/hostname");
            assert!(d.mode_string.starts_with('-'));
            assert!(!d.mode_string.is_empty());
            // /etc/hostname is owned by root (uid 0) on every distro.
            assert_eq!(d.owner_uid, 0);
        }
    }
}
