use crate::types::PluginId;
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use chrono::NaiveDate;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

const LOCK_FILE_NAME: &str = ".arclain-plugin-log-retention.lock";
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

const DEFAULT_MAX_FILES_PER_PLUGIN: usize = 14;
const DEFAULT_MAX_AGE: Duration = Duration::from_secs(30 * SECONDS_PER_DAY);
const DEFAULT_MAX_BYTES_PER_PLUGIN: u64 = 200 * 1024 * 1024;
const DEFAULT_MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
const DEFAULT_CLEANUP_INTERVAL: Duration = Duration::from_secs(15 * 60);
const DEFAULT_MAX_SCAN_ENTRIES: usize = 10_000;

/// Bounds retained plugin logs independently from the per-day write cap.
/// Cleanup is best-effort and never removes the current day's files.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PluginLogRetentionPolicy {
    pub(crate) max_files_per_plugin: usize,
    pub(crate) max_age: Duration,
    pub(crate) max_bytes_per_plugin: u64,
    pub(crate) max_total_bytes: u64,
    pub(crate) cleanup_interval: Duration,
    pub(crate) max_scan_entries: usize,
}

impl Default for PluginLogRetentionPolicy {
    fn default() -> Self {
        Self {
            max_files_per_plugin: DEFAULT_MAX_FILES_PER_PLUGIN,
            max_age: DEFAULT_MAX_AGE,
            max_bytes_per_plugin: DEFAULT_MAX_BYTES_PER_PLUGIN,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            cleanup_interval: DEFAULT_CLEANUP_INTERVAL,
            max_scan_entries: DEFAULT_MAX_SCAN_ENTRIES,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct RetentionRoot {
    configured: PathBuf,
    canonical: PathBuf,
    identity: RootIdentity,
}

impl RetentionRoot {
    pub(super) fn prepare(configured: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(configured)?;
        validate_directory(configured)?;
        let canonical = configured.canonicalize()?;
        validate_directory(&canonical)?;
        let identity = directory_identity(&canonical)?;
        Ok(Self {
            configured: configured.to_path_buf(),
            canonical,
            identity,
        })
    }

    pub(super) fn open_daily_log(&self, name: &str) -> io::Result<std::fs::File> {
        let root = self.open_validated()?;
        let mut options = OpenOptions::new();
        options
            .write(true)
            .append(true)
            .create(true)
            .follow(FollowSymlinks::No);
        let file = root.open_with(name, &options)?;
        if !is_regular_non_reparse(&file.metadata()?) {
            return Err(io::Error::other(
                "plugin log target is not a non-reparse regular file",
            ));
        }
        Ok(file.into_std())
    }

    /// Open a no-follow handle used to serialize retention and capped appends
    /// across every logger instance sharing this root.
    pub(super) fn open_coordination_lock(&self) -> io::Result<std::fs::File> {
        let root = self.open_validated()?;
        open_lock_file(&root)
    }

    pub(super) fn cleanup(
        &self,
        plugin_id: &PluginId,
        today: NaiveDate,
        policy: PluginLogRetentionPolicy,
    ) -> io::Result<()> {
        let root = self.open_validated()?;
        let lock = open_lock_file(&root)?;
        match std::fs::File::try_lock(&lock) {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(()),
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }

        // Revalidate after acquiring the inter-process lock. A configured
        // directory may have been replaced since the first handle was opened.
        let result = self
            .open_validated()
            .and_then(|root| cleanup_locked(&root, plugin_id, today, policy));
        let unlock = std::fs::File::unlock(&lock);
        result.and(unlock)
    }

    fn open_validated(&self) -> io::Result<Dir> {
        self.open_validated_with_hook(|| {})
    }

    fn open_validated_with_hook(&self, before_open: impl FnOnce()) -> io::Result<Dir> {
        validate_directory(&self.configured)?;
        let current = self.configured.canonicalize()?;
        if current != self.canonical {
            return Err(io::Error::other("plugin log directory identity changed"));
        }
        validate_directory(&current)?;
        before_open();
        let directory = Dir::open_ambient_dir(&current, cap_std::ambient_authority())?;
        if directory_handle_identity(&directory)? != self.identity {
            return Err(io::Error::other(
                "plugin log directory filesystem identity changed",
            ));
        }
        Ok(directory)
    }

    #[cfg(test)]
    pub(super) fn open_validated_with_hook_for_test(
        &self,
        before_open: impl FnOnce(),
    ) -> io::Result<Dir> {
        self.open_validated_with_hook(before_open)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RootIdentity {
    device: u64,
    file: u64,
}

fn directory_identity(path: &Path) -> io::Result<RootIdentity> {
    // `dir_metadata` is queried from an open directory handle. That provides
    // stable device/file identifiers on both Unix and Windows without the
    // unstable path-based Windows metadata APIs.
    let directory = Dir::open_ambient_dir(path, cap_std::ambient_authority())?;
    directory_handle_identity(&directory)
}

fn directory_handle_identity(directory: &Dir) -> io::Result<RootIdentity> {
    use cap_fs_ext::MetadataExt;

    let metadata = directory.dir_metadata()?;
    Ok(RootIdentity {
        device: metadata.dev(),
        file: metadata.ino(),
    })
}

#[derive(Debug)]
struct RetainedLog {
    name: OsString,
    display_name: String,
    plugin_id: String,
    date: NaiveDate,
    size: u64,
    deleted: bool,
}

fn cleanup_locked(
    root: &Dir,
    current_plugin: &PluginId,
    today: NaiveDate,
    policy: PluginLogRetentionPolicy,
) -> io::Result<()> {
    let mut logs = scan_logs(root, policy.max_scan_entries)?;
    logs.sort_by(|left, right| {
        (left.date, &left.display_name).cmp(&(right.date, &right.display_name))
    });

    let current_identity = current_plugin.as_str();
    for index in 0..logs.len() {
        if logs[index].plugin_id.eq_ignore_ascii_case(current_identity)
            && is_expired(logs[index].date, today, policy.max_age)
        {
            remove_candidate(root, &mut logs[index])?;
        }
    }

    enforce_plugin_limits(root, &mut logs, current_identity, today, policy)?;
    enforce_global_limit(root, &mut logs, today, policy.max_total_bytes)
}

fn scan_logs(root: &Dir, max_entries: usize) -> io::Result<Vec<RetainedLog>> {
    let mut names = Vec::new();
    for entry in root.entries()? {
        let entry = entry?;
        if names.len() >= max_entries {
            return Err(io::Error::other(
                "plugin log directory scan entry limit exceeded",
            ));
        }
        names.push(entry.file_name());
    }

    let mut logs = Vec::new();
    for name in names {
        let Some(display_name) = name.to_str().map(str::to_owned) else {
            continue;
        };
        let Some((plugin_id, date)) = parse_owned_log_name(&display_name) else {
            continue;
        };
        let metadata = match root.symlink_metadata(&name) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if !is_regular_non_reparse(&metadata) {
            continue;
        }
        logs.push(RetainedLog {
            name,
            display_name,
            plugin_id,
            date,
            size: metadata.len(),
            deleted: false,
        });
    }
    Ok(logs)
}

fn parse_owned_log_name(name: &str) -> Option<(String, NaiveDate)> {
    let stem = name.strip_suffix(".log")?;
    let separator = stem.len().checked_sub(11)?;
    if stem.as_bytes().get(separator) != Some(&b'-') {
        return None;
    }
    let plugin = &stem[..separator];
    let date_text = &stem[separator + 1..];
    let plugin_id = PluginId::parse(plugin.to_owned()).ok()?;
    let date = NaiveDate::parse_from_str(date_text, "%Y-%m-%d").ok()?;
    if date.format("%Y-%m-%d").to_string() != date_text {
        return None;
    }
    Some((plugin_id.as_str().to_owned(), date))
}

fn is_expired(date: NaiveDate, today: NaiveDate, max_age: Duration) -> bool {
    let age_days = (today - date).num_days();
    let Ok(age_days) = u64::try_from(age_days) else {
        return false;
    };
    age_days
        .checked_mul(SECONDS_PER_DAY)
        .is_some_and(|age| age > max_age.as_secs())
}

fn enforce_plugin_limits(
    root: &Dir,
    logs: &mut [RetainedLog],
    plugin_id: &str,
    today: NaiveDate,
    policy: PluginLogRetentionPolicy,
) -> io::Result<()> {
    loop {
        let (count, bytes) = plugin_totals(logs, plugin_id)?;
        if count <= policy.max_files_per_plugin && bytes <= policy.max_bytes_per_plugin {
            return Ok(());
        }
        let Some(candidate) = logs.iter_mut().find(|log| {
            !log.deleted && log.date != today && log.plugin_id.eq_ignore_ascii_case(plugin_id)
        }) else {
            return Ok(());
        };
        remove_candidate(root, candidate)?;
    }
}

fn enforce_global_limit(
    root: &Dir,
    logs: &mut [RetainedLog],
    today: NaiveDate,
    max_total_bytes: u64,
) -> io::Result<()> {
    loop {
        let total = checked_sum(logs.iter().filter(|log| !log.deleted).map(|log| log.size))?;
        if total <= max_total_bytes {
            return Ok(());
        }
        let Some(candidate) = logs
            .iter_mut()
            .find(|log| !log.deleted && log.date != today)
        else {
            return Ok(());
        };
        remove_candidate(root, candidate)?;
    }
}

fn plugin_totals(logs: &[RetainedLog], plugin_id: &str) -> io::Result<(usize, u64)> {
    let matching = logs
        .iter()
        .filter(|log| !log.deleted && log.plugin_id.eq_ignore_ascii_case(plugin_id));
    let mut count = 0usize;
    let mut bytes = 0u64;
    for log in matching {
        count = count
            .checked_add(1)
            .ok_or_else(|| io::Error::other("plugin log file count overflow"))?;
        bytes = bytes
            .checked_add(log.size)
            .ok_or_else(|| io::Error::other("plugin log byte accounting overflow"))?;
    }
    Ok((count, bytes))
}

fn checked_sum(mut sizes: impl Iterator<Item = u64>) -> io::Result<u64> {
    sizes.try_fold(0u64, |total, size| {
        total
            .checked_add(size)
            .ok_or_else(|| io::Error::other("plugin log byte accounting overflow"))
    })
}

fn remove_candidate(root: &Dir, candidate: &mut RetainedLog) -> io::Result<()> {
    let metadata = match root.symlink_metadata(&candidate.name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            candidate.deleted = true;
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    if !is_regular_non_reparse(&metadata) || metadata.len() != candidate.size {
        return Err(io::Error::other(
            "plugin log changed during retention cleanup",
        ));
    }
    root.remove_file(&candidate.name)?;
    candidate.deleted = true;
    Ok(())
}

fn open_lock_file(root: &Dir) -> io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(true)
        .follow(FollowSymlinks::No);
    let lock = root.open_with(LOCK_FILE_NAME, &options)?;
    if !is_regular_non_reparse(&lock.metadata()?) {
        return Err(io::Error::other(
            "plugin log retention lock is not a non-reparse regular file",
        ));
    }
    Ok(lock.into_std())
}

fn validate_directory(path: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || is_std_reparse(&metadata) {
        return Err(io::Error::other(
            "plugin log root is not a non-reparse directory",
        ));
    }
    Ok(())
}

fn is_regular_non_reparse(metadata: &cap_std::fs::Metadata) -> bool {
    metadata.is_file() && !is_cap_reparse(metadata)
}

#[cfg(windows)]
fn is_cap_reparse(metadata: &cap_std::fs::Metadata) -> bool {
    use cap_fs_ext::OsMetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_cap_reparse(metadata: &cap_std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
fn is_std_reparse(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_std_reparse(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}
