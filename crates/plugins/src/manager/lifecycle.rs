//! Plugin lifecycle management (loading, unloading, installation)

use super::types::ManagedPlugin;
use super::PluginManager;
use crate::loader::{DiscoveredPlugin, TrustedPluginRoot};
use crate::types::{
    CapabilitiesConfig, PluginError, PluginId, PluginIdentityKey, PluginInfoConfig, PluginManifest,
    PluginMetadata, RateLimits, Result,
};
#[cfg(not(windows))]
use cap_fs_ext::DirExt;
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
#[cfg(windows)]
use cap_fs_ext::{MetadataExt as CapMetadataExt, OpenOptionsMaybeDirExt, OsMetadataExt};
#[cfg(windows)]
use cap_std::fs::OpenOptionsExt as CapOpenOptionsExt;
use cap_std::fs::{Dir, OpenOptions};
use parking_lot::Mutex;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

pub(super) fn manifest_from_metadata(
    plugin_id: &PluginId,
    metadata: PluginMetadata,
) -> PluginManifest {
    PluginManifest {
        plugin: PluginInfoConfig {
            id: plugin_id.as_str().to_string(),
            name: metadata.name,
            version: metadata.version,
            author: metadata.author,
            description: metadata.description,
        },
        capabilities: CapabilitiesConfig::default(),
        rate_limits: RateLimits {
            http_requests_per_minute: 60,
        },
    }
}

pub(super) fn serialize_manifest(manifest: &PluginManifest) -> Result<String> {
    toml::to_string_pretty(manifest).map_err(|error| {
        PluginError::InvalidManifest(format!("Failed to serialize plugin manifest: {error}"))
    })
}

pub(super) struct StagedPluginPackage {
    trusted_root: Arc<TrustedPluginRoot>,
    staging_name: String,
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_vendor = "apple",
        target_os = "redox"
    ))]
    staging_dir: Dir,
    #[cfg(windows)]
    staging_dir: Option<Dir>,
    #[cfg(windows)]
    artifact_dir: Option<Dir>,
    #[cfg(windows)]
    staging_identity: WindowsDirectoryIdentity,
    #[cfg(windows)]
    artifact_identity: WindowsDirectoryIdentity,
    staging_path: PathBuf,
    artifact: PathBuf,
    identity_key: PluginIdentityKey,
    published: bool,
}

impl StagedPluginPackage {
    pub(super) fn new(
        trusted_root: Arc<TrustedPluginRoot>,
        plugin_id: &PluginId,
        wasm_bytes: &[u8],
        manifest: &PluginManifest,
    ) -> Result<Self> {
        trusted_root.revalidate_current_path()?;
        let staging_name = create_staging_directory(trusted_root.dir())?;
        let staging_dir =
            open_staged_directory(trusted_root.dir(), &staging_name, false).map_err(|error| {
                PluginError::LoadError(format!("Failed to open plugin staging directory: {error}"))
            })?;
        let identity_key = plugin_id.identity_key();
        staging_dir
            .create_dir(identity_key.as_str())
            .map_err(|error| {
                PluginError::LoadError(format!("Failed to create staged plugin directory: {error}"))
            })?;
        let artifact_dir = open_staged_directory(&staging_dir, identity_key.as_str(), false)
            .map_err(|error| {
                PluginError::LoadError(format!("Failed to open staged plugin directory: {error}"))
            })?;
        let staging_path = trusted_root.configured_path().join(&staging_name);
        let artifact_relative = PathBuf::from(&staging_name).join(identity_key.as_str());
        let artifact = trusted_root.configured_path().join(&artifact_relative);
        write_new_file(
            &artifact_dir,
            &format!("{}.wasm", identity_key.as_str()),
            wasm_bytes,
            "plugin WASM",
        )?;
        let manifest_content = serialize_manifest(manifest)?;
        write_new_file(
            &artifact_dir,
            &format!("{}.toml", identity_key.as_str()),
            manifest_content.as_bytes(),
            "plugin manifest",
        )?;

        #[cfg(windows)]
        let staging_identity =
            windows_directory_identity(&staging_dir, "plugin staging directory")?;
        #[cfg(windows)]
        let artifact_identity =
            windows_directory_identity(&artifact_dir, "staged plugin directory")?;

        let staged = Self {
            trusted_root,
            staging_name,
            #[cfg(any(
                target_os = "linux",
                target_os = "android",
                target_vendor = "apple",
                target_os = "redox"
            ))]
            staging_dir,
            #[cfg(windows)]
            staging_dir: Some(staging_dir),
            #[cfg(windows)]
            artifact_dir: Some(artifact_dir),
            #[cfg(windows)]
            staging_identity,
            #[cfg(windows)]
            artifact_identity,
            staging_path,
            artifact,
            identity_key,
            published: false,
        };

        Ok(staged)
    }

    #[cfg(test)]
    pub(super) fn root_path(&self) -> &Path {
        &self.staging_path
    }

    fn manifest_path(&self) -> PathBuf {
        self.artifact
            .join(format!("{}.toml", self.identity_key.as_str()))
    }

    pub(super) fn publish(mut self, destination: &Path) -> Result<()> {
        self.validate_publish_destination(destination)?;
        self.prepare_windows_artifact_for_mutation()?;
        rename_staged_no_replace(&self)?;
        self.published = true;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn publish_with_before_rename(
        mut self,
        destination: &Path,
        before_rename: impl FnOnce(&Self) -> std::io::Result<()>,
    ) -> Result<()> {
        self.validate_publish_destination(destination)?;
        self.prepare_windows_artifact_for_mutation()?;
        before_rename(&self).map_err(|error| {
            PluginError::LoadError(format!("Injected pre-publish failure: {error}"))
        })?;
        rename_staged_no_replace(&self)?;
        self.published = true;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn rollback_with_before_cleanup(
        mut self,
        before_cleanup: impl FnOnce(&Self),
    ) -> Result<()> {
        self.prepare_windows_artifact_for_mutation()?;
        before_cleanup(&self);
        Ok(self.cleanup_staging()?)
    }

    #[cfg(test)]
    pub(super) fn artifact_path(&self) -> &Path {
        &self.artifact
    }

    #[cfg(all(test, windows))]
    pub(super) fn move_open_artifact_for_test(
        &self,
        destination_name: &std::ffi::OsStr,
    ) -> std::io::Result<()> {
        let artifact_dir = self.artifact_dir.as_ref().ok_or_else(|| {
            std::io::Error::other("staged plugin directory handle is unavailable")
        })?;
        let staging_dir = self.staging_dir.as_ref().ok_or_else(|| {
            std::io::Error::other("plugin staging directory handle is unavailable")
        })?;
        rename_windows_handle_relative_no_replace(artifact_dir, staging_dir, destination_name)
    }

    #[cfg(windows)]
    fn prepare_windows_artifact_for_mutation(&mut self) -> Result<()> {
        drop(self.artifact_dir.take());
        let staging_dir = self.staging_dir.as_ref().ok_or_else(|| {
            PluginError::LoadError("Plugin staging directory handle is unavailable".to_string())
        })?;
        let artifact_dir = open_staged_directory(staging_dir, self.identity_key.as_str(), true)
            .map_err(|error| {
                PluginError::LoadError(format!(
                    "Failed to open staged plugin directory for publication: {error}"
                ))
            })?;
        validate_windows_directory_handle(
            &artifact_dir,
            self.artifact_identity,
            "staged plugin directory",
        )
        .map_err(|error| {
            PluginError::LoadError(format!(
                "Failed to validate staged plugin directory for publication: {error}"
            ))
        })?;
        self.artifact_dir = Some(artifact_dir);
        Ok(())
    }

    #[cfg(windows)]
    fn prepare_windows_staging_for_deletion(&mut self) -> std::io::Result<()> {
        drop(self.staging_dir.take());
        let staging_dir = open_staged_directory(self.trusted_root.dir(), &self.staging_name, true)?;
        validate_windows_directory_handle(
            &staging_dir,
            self.staging_identity,
            "plugin staging directory entry",
        )?;
        self.staging_dir = Some(staging_dir);
        Ok(())
    }

    #[cfg(not(windows))]
    fn prepare_windows_artifact_for_mutation(&mut self) -> Result<()> {
        Ok(())
    }

    fn validate_publish_destination(&self, destination: &Path) -> Result<()> {
        let destination_parent = destination.parent().ok_or_else(|| {
            PluginError::LoadError("Plugin destination has no parent directory".to_string())
        })?;
        if destination_parent != self.trusted_root.configured_path()
            || destination.file_name() != Some(std::ffi::OsStr::new(self.identity_key.as_str()))
        {
            return Err(PluginError::LoadError(format!(
                "Plugin destination is outside its configured root: {}",
                destination.display()
            )));
        }
        self.trusted_root.revalidate_current_path()?;
        match self
            .trusted_root
            .dir()
            .symlink_metadata(self.identity_key.as_str())
        {
            Ok(_) => {
                return Err(PluginError::LoadError(format!(
                    "Plugin destination already exists: {}",
                    destination.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(PluginError::LoadError(format!(
                    "Failed to inspect plugin destination {}: {error}",
                    destination.display()
                )));
            }
        }
        Ok(())
    }
}

impl Drop for StagedPluginPackage {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup_staging() {
            if error.kind() != std::io::ErrorKind::NotFound {
                warn!(
                    staging_dir = %self.staging_path.display(),
                    %error,
                    "Failed to clean up plugin staging directory"
                );
            }
        }
    }
}

impl StagedPluginPackage {
    #[cfg(windows)]
    fn cleanup_staging(&mut self) -> std::io::Result<()> {
        {
            let Some(staging_dir) = self.staging_dir.as_ref() else {
                return Ok(());
            };
            validate_windows_directory_handle(
                staging_dir,
                self.staging_identity,
                "plugin staging directory",
            )?;
        }

        if self.published {
            if let Some(artifact_dir) = self.artifact_dir.take() {
                validate_windows_directory_handle(
                    &artifact_dir,
                    self.artifact_identity,
                    "published plugin directory",
                )?;
                drop(artifact_dir);
            }
            let staging_dir = self.staging_dir.as_ref().ok_or_else(|| {
                std::io::Error::other("plugin staging directory handle is unavailable")
            })?;
            ensure_windows_directory_empty(staging_dir, "plugin staging directory")?;
        } else {
            self.prepare_windows_artifact_for_mutation()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            let staging_dir = self.staging_dir.as_ref().ok_or_else(|| {
                std::io::Error::other("plugin staging directory handle is unavailable")
            })?;
            let artifact_dir = self.artifact_dir.as_ref().ok_or_else(|| {
                std::io::Error::other("staged plugin directory handle is unavailable")
            })?;
            validate_windows_directory_handle(
                artifact_dir,
                self.artifact_identity,
                "staged plugin directory",
            )?;
            cleanup_windows_staged_files(artifact_dir, &self.identity_key)?;

            let artifact_dir = self.artifact_dir.take().ok_or_else(|| {
                std::io::Error::other("staged plugin directory handle is unavailable")
            })?;
            mark_windows_handle_for_deletion(&artifact_dir)?;
            drop(artifact_dir);
            ensure_windows_directory_empty(staging_dir, "plugin staging directory")?;
        }

        self.prepare_windows_staging_for_deletion()?;
        let staging_dir = self.staging_dir.take().ok_or_else(|| {
            std::io::Error::other("plugin staging directory handle is unavailable")
        })?;
        mark_windows_handle_for_deletion(&staging_dir)?;
        drop(staging_dir);
        Ok(())
    }

    #[cfg(not(windows))]
    fn cleanup_staging(&mut self) -> std::io::Result<()> {
        self.trusted_root.dir().remove_dir_all(&self.staging_name)
    }
}

static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(0);

fn create_staging_directory(root: &Dir) -> Result<String> {
    for _ in 0..128 {
        let sequence = NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed);
        let name = format!(
            ".arclain-plugin-install-{}-{sequence:016x}",
            std::process::id()
        );
        match root.create_dir(&name) {
            Ok(()) => return Ok(name),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(PluginError::LoadError(format!(
                    "Failed to create plugin staging directory: {error}"
                )));
            }
        }
    }
    Err(PluginError::LoadError(
        "Failed to allocate a unique plugin staging directory".to_string(),
    ))
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WindowsDirectoryIdentity {
    device: u64,
    inode: u64,
}

#[cfg(windows)]
fn windows_directory_identity(
    directory: &Dir,
    kind: &str,
) -> std::io::Result<WindowsDirectoryIdentity> {
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    let metadata = directory.dir_metadata()?;
    if !metadata.is_dir()
        || OsMetadataExt::file_attributes(&metadata) & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(std::io::Error::other(format!(
            "{kind} is not a non-reparse directory"
        )));
    }
    Ok(WindowsDirectoryIdentity {
        device: CapMetadataExt::dev(&metadata),
        inode: CapMetadataExt::ino(&metadata),
    })
}

#[cfg(windows)]
fn validate_windows_directory_handle(
    directory: &Dir,
    expected: WindowsDirectoryIdentity,
    kind: &str,
) -> std::io::Result<()> {
    let actual = windows_directory_identity(directory, kind)?;
    if actual != expected {
        return Err(std::io::Error::other(format!(
            "{kind} identity changed after it was opened"
        )));
    }
    Ok(())
}

#[cfg(windows)]
fn open_staged_directory(
    parent: &Dir,
    name: impl AsRef<Path>,
    delete_access: bool,
) -> std::io::Result<Dir> {
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY,
        FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, SYNCHRONIZE,
    };

    let mut options = OpenOptions::new();
    let mut access_mode = FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
    if delete_access {
        access_mode |= DELETE;
    }
    options
        .access_mode(access_mode)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .follow(FollowSymlinks::No)
        .maybe_dir(true);
    let file = parent.open_with(name, &options)?;
    let directory = Dir::from_std_file(file.into_std());
    windows_directory_identity(&directory, "staged directory")?;
    Ok(directory)
}

#[cfg(not(windows))]
fn open_staged_directory(
    parent: &Dir,
    name: impl AsRef<Path>,
    _delete_access: bool,
) -> std::io::Result<Dir> {
    parent.open_dir_nofollow(name)
}

#[cfg(windows)]
fn ensure_windows_directory_empty(directory: &Dir, kind: &str) -> std::io::Result<()> {
    if directory.entries()?.next().transpose()?.is_some() {
        return Err(std::io::Error::other(format!(
            "{kind} contains an unexpected entry; refusing recursive cleanup"
        )));
    }
    Ok(())
}

#[cfg(windows)]
fn cleanup_windows_staged_files(
    artifact_dir: &Dir,
    identity_key: &PluginIdentityKey,
) -> std::io::Result<()> {
    let mut actual = artifact_dir
        .entries()?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    actual.sort();
    let mut expected = vec![
        std::ffi::OsString::from(format!("{}.toml", identity_key.as_str())),
        std::ffi::OsString::from(format!("{}.wasm", identity_key.as_str())),
    ];
    expected.sort();
    if actual != expected {
        return Err(std::io::Error::other(
            "staged plugin directory contents changed; refusing cleanup",
        ));
    }

    for file_name in expected {
        let file = open_windows_staged_file_for_deletion(artifact_dir, &file_name)?;
        mark_windows_handle_for_deletion(&file)?;
        drop(file);
    }
    ensure_windows_directory_empty(artifact_dir, "staged plugin directory")
}

#[cfg(windows)]
fn open_windows_staged_file_for_deletion(
    parent: &Dir,
    name: &std::ffi::OsStr,
) -> std::io::Result<cap_std::fs::File> {
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, SYNCHRONIZE,
    };

    let mut options = OpenOptions::new();
    options
        .access_mode(DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .follow(FollowSymlinks::No);
    let file = parent.open_with(name, &options)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || OsMetadataExt::file_attributes(&metadata) & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(std::io::Error::other(
            "staged plugin file is not a non-reparse regular file",
        ));
    }
    Ok(file)
}

#[cfg(windows)]
fn mark_windows_handle_for_deletion(
    handle: &impl std::os::windows::io::AsRawHandle,
) -> std::io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfoEx, SetFileInformationByHandle, FILE_DISPOSITION_FLAG_DELETE,
        FILE_DISPOSITION_INFO_EX,
    };

    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE,
    };
    // SAFETY: `handle` is live for the call and `disposition` has the exact
    // layout and byte size required by FileDispositionInfoEx.
    let deleted = unsafe {
        SetFileInformationByHandle(
            handle.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE,
            FileDispositionInfoEx,
            (&disposition as *const FILE_DISPOSITION_INFO_EX).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    };
    if deleted == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn write_new_file(directory: &Dir, name: &str, bytes: &[u8], kind: &str) -> Result<()> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    let mut file = directory.open_with(name, &options).map_err(|error| {
        PluginError::LoadError(format!("Failed to create staged {kind}: {error}"))
    })?;
    file.write_all(bytes).map_err(|error| {
        PluginError::LoadError(format!("Failed to write staged {kind}: {error}"))
    })?;
    file.sync_all()
        .map_err(|error| PluginError::LoadError(format!("Failed to flush staged {kind}: {error}")))
}

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "redox"
))]
fn rename_staged_no_replace(staged: &StagedPluginPackage) -> Result<()> {
    rustix::fs::renameat_with(
        &staged.staging_dir,
        staged.identity_key.as_str(),
        staged.trusted_root.dir(),
        staged.identity_key.as_str(),
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(std::io::Error::from)
    .map_err(|error| {
        PluginError::LoadError(format!(
            "Failed to atomically publish plugin to {}: {error}",
            staged
                .trusted_root
                .configured_path()
                .join(staged.identity_key.as_str())
                .display()
        ))
    })
}

#[cfg(windows)]
fn rename_staged_no_replace(staged: &StagedPluginPackage) -> Result<()> {
    let staging_dir = staged.staging_dir.as_ref().ok_or_else(|| {
        PluginError::LoadError("Plugin staging directory handle is unavailable".to_string())
    })?;
    let artifact_dir = staged.artifact_dir.as_ref().ok_or_else(|| {
        PluginError::LoadError("Staged plugin directory handle is unavailable".to_string())
    })?;
    validate_windows_directory_handle(
        staging_dir,
        staged.staging_identity,
        "plugin staging directory",
    )
    .map_err(|error| {
        PluginError::LoadError(format!(
            "Failed to validate plugin staging directory: {error}"
        ))
    })?;
    validate_windows_directory_handle(
        artifact_dir,
        staged.artifact_identity,
        "staged plugin directory",
    )
    .map_err(|error| {
        PluginError::LoadError(format!(
            "Failed to validate staged plugin directory: {error}"
        ))
    })?;
    rename_windows_handle_relative_no_replace(
        artifact_dir,
        staged.trusted_root.dir(),
        std::ffi::OsStr::new(staged.identity_key.as_str()),
    )
    .map_err(|error| {
        PluginError::LoadError(format!(
            "Failed to atomically publish plugin to {}: {error}",
            staged
                .trusted_root
                .configured_path()
                .join(staged.identity_key.as_str())
                .display()
        ))
    })
}

#[cfg(windows)]
pub(super) fn rename_windows_handle_relative_no_replace(
    source: &impl std::os::windows::io::AsRawHandle,
    destination_parent: &impl std::os::windows::io::AsRawHandle,
    destination_name: &std::ffi::OsStr,
) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Wdk::Storage::FileSystem::{
        FileRenameInformation, NtSetInformationFile, FILE_RENAME_INFORMATION,
    };
    use windows_sys::Win32::Foundation::RtlNtStatusToDosError;
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    let destination_name = destination_name.encode_wide().collect::<Vec<_>>();
    let header_size = std::mem::offset_of!(FILE_RENAME_INFORMATION, FileName);
    let buffer_size = header_size
        .checked_add(destination_name.len() * std::mem::size_of::<u16>())
        .ok_or_else(|| std::io::Error::other("Plugin destination name is too long"))?;
    let word_size = std::mem::size_of::<usize>();
    let mut buffer = vec![0_usize; buffer_size.div_ceil(word_size)];
    let rename_info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();

    // SAFETY: `buffer` is aligned for `FILE_RENAME_INFORMATION`, is large enough for
    // the fixed fields and UTF-16 name, and remains live for the API call.
    unsafe {
        (*rename_info).Anonymous.ReplaceIfExists = false;
        (*rename_info).RootDirectory = destination_parent.as_raw_handle().cast();
        (*rename_info).FileNameLength =
            (destination_name.len() * std::mem::size_of::<u16>()) as u32;
        std::ptr::copy_nonoverlapping(
            destination_name.as_ptr(),
            std::ptr::addr_of_mut!((*rename_info).FileName).cast::<u16>(),
            destination_name.len(),
        );
    }

    let mut io_status = IO_STATUS_BLOCK::default();
    // SAFETY: both directory handles are live, `io_status` is writable, and
    // `rename_info` points to the initialized variable-length buffer described
    // above. FileRenameInformation with ReplaceIfExists=false makes publication
    // atomic and non-overwriting while resolving the name from the root handle.
    let status = unsafe {
        NtSetInformationFile(
            source.as_raw_handle().cast(),
            &mut io_status,
            rename_info.cast(),
            buffer_size as u32,
            FileRenameInformation,
        )
    };
    if status < 0 {
        // SAFETY: `status` is the NTSTATUS returned by NtSetInformationFile.
        let win32_error = unsafe { RtlNtStatusToDosError(status) };
        return Err(std::io::Error::from_raw_os_error(win32_error as i32));
    }
    Ok(())
}

#[cfg(not(any(
    windows,
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "redox"
)))]
fn rename_staged_no_replace(staged: &StagedPluginPackage) -> Result<()> {
    arclain_core::utilities::rename_no_replace(
        &staged.artifact,
        &staged
            .trusted_root
            .configured_path()
            .join(staged.identity_key.as_str()),
    )
    .map_err(|error| {
        PluginError::LoadError(format!("Failed to atomically publish plugin: {error}"))
    })
}

fn on_disk_identity_collision(
    trusted_root: &TrustedPluginRoot,
    identity_key: &PluginIdentityKey,
) -> Result<Option<PathBuf>> {
    let entries = trusted_root.dir().entries().map_err(|error| {
        PluginError::LoadError(format!(
            "Failed to scan plugin root {}: {error}",
            trusted_root.configured_path().display()
        ))
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            PluginError::LoadError(format!("Failed to inspect plugin root entry: {error}"))
        })?;
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        let entry_path = PathBuf::from(&file_name);
        let candidate = entry_path
            .extension()
            .and_then(|extension| extension.to_str())
            .filter(|extension| {
                extension.eq_ignore_ascii_case("toml") || extension.eq_ignore_ascii_case("wasm")
            })
            .and_then(|_| entry_path.file_stem().and_then(|stem| stem.to_str()))
            .unwrap_or(file_name);
        let Ok(candidate_id) = PluginId::parse(candidate.to_string()) else {
            continue;
        };
        if candidate_id.identity_key() == *identity_key {
            return Ok(Some(trusted_root.configured_path().join(entry_path)));
        }
    }
    Ok(None)
}

impl PluginManager {
    /// Initialize the plugin manager and load discovered plugins
    pub fn init(&mut self) -> Result<()> {
        let plugins = self.loader.discover_plugins()?;
        for plugin in plugins {
            match self.load_plugin(&plugin) {
                Ok(_) => debug!("Loaded plugin: {}", plugin.manifest.plugin.id),
                Err(e) => error!("Failed to load plugin {}: {}", plugin.manifest.plugin.id, e),
            }
        }
        Ok(())
    }

    /// Load a single plugin
    pub(crate) fn load_plugin(&mut self, discovered: &DiscoveredPlugin) -> Result<()> {
        let plugin_id = discovered.manifest.plugin.id.clone();
        let identity_key = PluginId::parse(plugin_id.clone())?.identity_key();

        // Check if already loaded
        if self.is_identity_registered(&identity_key) {
            return Err(PluginError::LoadError(format!(
                "Plugin already loaded: {}",
                plugin_id
            )));
        }

        let managed = self.prepare_plugin(discovered)?;
        let plugin_name = managed.metadata.name.clone();
        self.register_prepared_plugin(identity_key, managed);

        info!("Plugin '{}' loaded and initialized", plugin_name);
        Ok(())
    }

    fn prepare_plugin(&self, discovered: &DiscoveredPlugin) -> Result<ManagedPlugin> {
        let plugin_id = discovered.manifest.plugin.id.clone();
        let identity_key = PluginIdentityKey::parse(&plugin_id)?;

        // Load the WASM module
        let loaded = self.loader.load_plugin(discovered)?;

        // Get capabilities from manifest
        let capabilities = discovered.manifest.capabilities.to_capabilities();

        // Get rate limit from manifest
        let rate_limit = discovered.manifest.rate_limits.http_requests_per_minute;

        // Get initial settings for this plugin
        let settings = self
            .initial_settings
            .get(&identity_key)
            .map(|entry| entry.values.clone())
            .unwrap_or_default();

        // Instantiate the plugin with its host-function state.
        let mut instance = loaded.instantiate_with_plugin_log_dir(
            capabilities.clone(),
            rate_limit,
            self.library_service.clone(),
            settings,
            self.active_tab_bridge.clone(),
            &self.plugin_log_dir,
        )?;

        // Inject optional services
        if let Some(ref client) = self.gameta_client {
            instance.set_gameta_client(Some(client.clone()));
        }
        if let Some(ref client) = self.async_http_client {
            client.configure_plugin(
                &plugin_id,
                arclain_network::PluginNetworkPolicy {
                    network_enabled: capabilities
                        .contains(&crate::types::PluginCapability::Network),
                    requests_per_minute: rate_limit,
                },
            );
            client.replace_plugin_manifest_domains(
                &plugin_id,
                &discovered.manifest.capabilities.network_domains,
            );
            instance.set_async_http_client(Some(client.clone()));
        }

        // Initialize the plugin
        if let Err(error) = instance.init() {
            if let Some(ref client) = self.async_http_client {
                client.remove_plugin_configuration(&plugin_id);
            }
            return Err(error);
        }

        // Get metadata from manifest (WIT get_metadata is not yet implemented and returns defaults)
        let manifest = &discovered.manifest;
        let metadata = PluginMetadata {
            id: manifest.plugin.id.clone(),
            name: manifest.plugin.name.clone(),
            version: manifest.plugin.version.clone(),
            description: manifest.plugin.description.clone(),
            author: manifest.plugin.author.clone(),
        };

        // Snapshot the dirty handle BEFORE moving the instance into the
        // Arc<Mutex<...>> — saves a redundant lock just to clone an Arc.
        let settings_dirty = instance.settings_dirty_handle();

        // Create managed plugin
        let managed = ManagedPlugin {
            metadata: metadata.clone(),
            instance: Arc::new(Mutex::new(instance)),
            manifest: discovered.manifest.clone(),
            enabled: true,
            settings_dirty,
        };

        Ok(managed)
    }

    fn is_identity_registered(&self, identity_key: &PluginIdentityKey) -> bool {
        self.plugins.read().contains_key(identity_key)
    }

    fn register_prepared_plugin(&self, identity_key: PluginIdentityKey, managed: ManagedPlugin) {
        self.plugins.write().insert(identity_key.clone(), managed);
        self.enabled_plugins.write().insert(identity_key, true);
        self.invalidate_top_tabs_cache();
    }

    fn discard_prepared_plugin(&self, plugin_id: &str, managed: ManagedPlugin) {
        if let Some(ref client) = self.async_http_client {
            client.remove_plugin_configuration(plugin_id);
        }
        if let Err(error) = managed.instance.lock().cleanup() {
            warn!(
                "Failed to clean up plugin '{}' after installation rollback: {}",
                plugin_id, error
            );
        }
    }

    /// Reload a plugin
    pub fn reload_plugin(&mut self, plugin_id: &str) -> Result<()> {
        info!("Reloading plugin: {}", plugin_id);
        let identity_key = PluginIdentityKey::parse(plugin_id)
            .map_err(|_| PluginError::NotFound(plugin_id.to_string()))?;

        // Remove existing plugin
        let removed = self.plugins.write().remove(&identity_key);
        self.enabled_plugins.write().remove(&identity_key);
        self.settings_cache.lock().remove(&identity_key);
        if let Some(removed) = removed {
            let registered_id = removed.metadata.id.clone();
            if let Some(ref client) = self.async_http_client {
                client.remove_plugin_configuration(&registered_id);
            }
            removed.instance.lock().cleanup()?;
        }

        // Discover plugins again
        let discovered = self.loader.discover_plugins()?;

        // Find the plugin to reload
        let plugin_info = discovered
            .iter()
            .find(|plugin| {
                PluginIdentityKey::parse(&plugin.manifest.plugin.id)
                    .map(|candidate| candidate == identity_key)
                    .unwrap_or(false)
            })
            .ok_or_else(|| PluginError::NotFound(plugin_id.to_string()))?;

        // Load it
        self.load_plugin(plugin_info)?;

        info!("Plugin reloaded: {}", plugin_id);
        Ok(())
    }

    /// Unload a plugin
    pub fn unload_plugin(&mut self, plugin_id: &str) -> Result<()> {
        info!("Unloading plugin: {}", plugin_id);
        let identity_key = PluginIdentityKey::parse(plugin_id)
            .map_err(|_| PluginError::NotFound(plugin_id.to_string()))?;

        let plugin = self.plugins.write().remove(&identity_key);
        if let Some(plugin) = plugin {
            self.enabled_plugins.write().remove(&identity_key);
            self.settings_cache.lock().remove(&identity_key);
            let registered_id = plugin.metadata.id.clone();
            if let Some(ref client) = self.async_http_client {
                client.remove_plugin_configuration(&registered_id);
            }
            plugin.instance.lock().cleanup()?;
            self.invalidate_top_tabs_cache();
            info!("Plugin unloaded: {}", plugin_id);
            Ok(())
        } else {
            Err(PluginError::NotFound(plugin_id.to_string()))
        }
    }

    /// Install a plugin from a .wasm file
    ///
    /// This will:
    /// 1. Load and validate the plugin from the .wasm file
    /// 2. Extract metadata and create a manifest
    /// 3. Create a directory in plugins/ with the plugin ID
    /// 4. Copy `<id>.wasm` and create `<id>.toml`
    /// 5. Load the plugin into the manager
    pub fn install_plugin(&mut self, wasm_path: &std::path::Path) -> Result<String> {
        info!("Installing plugin from: {}", wasm_path.display());

        // Validate file exists and is a .wasm file
        if !wasm_path.exists() {
            return Err(PluginError::LoadError("File does not exist".to_string()));
        }

        if wasm_path.extension().and_then(|s| s.to_str()) != Some("wasm") {
            return Err(PluginError::LoadError(
                "File must be a .wasm file".to_string(),
            ));
        }

        // Bound the incoming component before allocating for it. The loader
        // also reads through the opened handle with a max+1 guard so a file
        // that grows after metadata inspection cannot bypass the limit.
        let wasm_bytes = self.loader.read_wasm_file(wasm_path)?;

        // Load the plugin to get metadata (without full instantiation)
        let loaded = self.loader.load_wasm(&wasm_bytes)?;

        // Metadata validation runs in a restricted host mode. It exposes only
        // the component's metadata export: plugin init is deferred, WASI does
        // not inherit process I/O, and every side-effecting host import is
        // denied or suppressed until the exported ID is validated.
        let mut temp_instance = loaded.instantiate_for_metadata_validation()?;
        let metadata_result = temp_instance.get_metadata();
        let cleanup_result = temp_instance.cleanup();
        let metadata = metadata_result?;
        cleanup_result?;
        let plugin_id = PluginId::parse(metadata.id.clone());
        let plugin_id = plugin_id?;
        let identity_key = plugin_id.identity_key();
        let manifest = manifest_from_metadata(&plugin_id, metadata);
        self.loader.validate_manifest(&manifest)?;

        // Check if plugin is already installed
        if self.is_identity_registered(&identity_key) {
            return Err(PluginError::LoadError(format!(
                "Plugin '{}' is already installed",
                plugin_id
            )));
        }

        let plugin_dir = identity_key.join_under(self.plugins_dir());
        if let Some(existing) =
            on_disk_identity_collision(&self.loader.trusted_root(), &identity_key)?
        {
            return Err(PluginError::LoadError(format!(
                "Plugin '{}' collides with an existing on-disk identity at {}",
                plugin_id,
                existing.display()
            )));
        }

        // Stage a complete package beside its destination. Discovery,
        // compilation and plugin initialization all operate on staged files;
        // the final publication is one atomic no-replace rename.
        let staged = StagedPluginPackage::new(
            self.loader.trusted_root(),
            &plugin_id,
            &wasm_bytes,
            &manifest,
        )?;
        let discovered = self
            .loader
            .discover_plugin_from_folder(&staged.manifest_path())?;
        let managed = self.prepare_plugin(&discovered)?;

        if let Err(error) = staged.publish(&plugin_dir) {
            self.discard_prepared_plugin(plugin_id.as_str(), managed);
            return Err(error);
        }

        let plugin_name = managed.metadata.name.clone();
        self.register_prepared_plugin(identity_key, managed);
        info!("Plugin files installed to: {}", plugin_dir.display());
        info!("Plugin '{}' loaded and initialized", plugin_name);

        info!("Plugin '{}' installed and loaded successfully", plugin_id);
        Ok(plugin_id.as_str().to_owned())
    }
}
