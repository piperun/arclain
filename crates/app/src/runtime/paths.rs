//! Where Arclain's on-disk state lives, and how that location is
//! resolved.
//!
//! Characterizes and replaces two things `crates/ui/src/core/state/
//! init.rs` used to do inline: calling `arclain_app_fs::AppDirectories
//! ::init` for the five base directories ("directories"), and a
//! separate algorithm for locating the plugin binaries directory
//! ("plugin directory resolution") that is deliberately independent of
//! `AppDirectories`'s own (unused) `plugins_dir` field -- see
//! [`resolve_plugins_dir`]'s doc comment.

use std::path::{Path, PathBuf};

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};

const APP_NAME: &str = "arclain";
const PLUGINS_DIR_ENV: &str = "ARCLAIN_PLUGINS_DIR";

/// The five on-disk directories `ArclainApp::bootstrap` resolves and
/// creates before anything else. All frontends read this back via
/// [`crate::ArclainApp::paths`] -- for example, a Settings page showing
/// "your data is stored at: ...".
///
/// `config_dir` and `data_dir` are deliberately two separate fields even
/// though [`AppPaths::system_default`] resolves them to the *same*
/// directory today: this app has never split "configuration" from
/// "persisted data" (databases, secrets) onto separate OS-conventional
/// roots the way, say, XDG_CONFIG_HOME/XDG_DATA_HOME differ on Linux --
/// both `config.sqlite` and the `databases`/`secrets` subdirectories
/// have always lived under the one directory `dirs::config_dir()`
/// resolves. Keeping the fields distinct in the type, while collapsing
/// them to one path under system defaults, describes that reality
/// honestly without redesigning it, and leaves room for a future
/// installation layout that genuinely separates them (`paths_override`
/// already allows a caller to supply two different directories today).
///
/// Databases live at `data_dir/databases/*` and `data_dir/secrets/*`
/// (`config.sqlite`, `metadata.sqlite`, `pass.redb`, `master.key`) --
/// see [`AppPaths::databases_dir`] / [`AppPaths::secrets_dir`].
#[derive(Clone, Debug)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub log_dir: PathBuf,
    pub plugins_dir: PathBuf,
}

fn directory_error(context: &str, error: impl std::fmt::Display) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Persistence,
        "failed to prepare application directories",
    )
    .with_diagnostic(format!("{context}: {error}"))
    .with_recoverability(Recoverability::Fatal)
}

impl AppPaths {
    /// Computes the OS-conventional default paths. Pure: performs no
    /// I/O and creates nothing on disk, so it is safe to call from
    /// anywhere (including a test) without touching a real profile.
    /// [`ensure_created`](Self::ensure_created) is the separate,
    /// side-effecting step that actually creates these directories --
    /// `ArclainApp::bootstrap` always calls both, in that order.
    pub fn system_default() -> Result<Self, ApplicationError> {
        // Mirrors the base-path half of `arclain_app_fs::AppDirectories
        // ::init` (falls back to the current directory if the OS can't
        // provide a home), without that function's other half: creating
        // the directories. `AppDirectories::init` intentionally couples
        // "compute" and "create" into one step for its own callers;
        // this crate needs them separable so path resolution stays
        // testable without side effects.
        let config_home = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        let cache_home = dirs::cache_dir().unwrap_or_else(|| PathBuf::from("."));
        let data_home = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));

        Ok(Self {
            config_dir: config_home.join(APP_NAME),
            data_dir: config_home.join(APP_NAME),
            cache_dir: cache_home.join(APP_NAME),
            log_dir: data_home.join(APP_NAME).join("logs"),
            plugins_dir: resolve_plugins_dir(),
        })
    }

    /// Resolves the paths this bootstrap runs against: `paths_override`
    /// verbatim if given, otherwise [`Self::system_default`].
    pub(crate) fn resolve(paths_override: Option<AppPaths>) -> Result<Self, ApplicationError> {
        match paths_override {
            Some(paths) => Ok(paths),
            None => Self::system_default(),
        }
    }

    /// Creates every directory named here (plus the `databases`/
    /// `secrets` subdirectories of `data_dir`) with owner-only
    /// permissions, via the same [`arclain_app_fs::ensure_owner_dir`]
    /// primitive `AppDirectories::init` uses. Idempotent -- safe to call
    /// against an already-initialized profile.
    ///
    /// `plugins_dir` is deliberately handled separately, outside this
    /// fatal loop -- see [`Self::prepare_plugins_dir`].
    pub(crate) fn ensure_created(&self) -> Result<(), ApplicationError> {
        for dir in [
            &self.config_dir,
            &self.data_dir,
            &self.cache_dir,
            &self.log_dir,
            &self.databases_dir(),
            &self.secrets_dir(),
            &self.materialization_dir(),
        ] {
            arclain_app_fs::ensure_owner_dir(dir)
                .map_err(|error| directory_error(&format!("creating {}", dir.display()), error))?;
        }
        self.prepare_plugins_dir();
        Ok(())
    }

    /// Best-effort creation of `plugins_dir`: attempts `create_dir_all`
    /// and warns (never fails bootstrap) if that doesn't work. Two
    /// deliberate differences from the other five directories above:
    ///
    /// - **Non-fatal.** Under a system install, `plugins_dir` can
    ///   legitimately be a directory this process does not own or
    ///   cannot write to (`/usr/lib/arclain/plugins`, `Program Files\
    ///   Arclain\plugins`) -- the directory already exists with real
    ///   plugin binaries in it, just not writable by whatever user
    ///   account is running the app. Bootstrap must still succeed with
    ///   plugin loading degraded, matching the behavior before this
    ///   directory was folded into `AppDirectories`-style fatal
    ///   creation.
    /// - **No `chmod`.** `ensure_owner_dir` also restricts a directory
    ///   to `0o700` (owner-only). Plugin `.wasm`/`.toml` files are not
    ///   secret material the way databases/keys are, and a system
    ///   install's plugins directory is typically meant to be
    ///   world-readable; forcing it owner-only would fight the
    ///   installer, not protect anything.
    fn prepare_plugins_dir(&self) {
        if let Err(error) = std::fs::create_dir_all(&self.plugins_dir) {
            tracing::warn!(
                "Failed to prepare plugins directory {}: {} -- plugin loading will be degraded",
                self.plugins_dir.display(),
                error
            );
        }
    }

    /// Where `config.sqlite` and `metadata.sqlite` live.
    pub(crate) fn databases_dir(&self) -> PathBuf {
        self.data_dir.join("databases")
    }

    /// Where `pass.redb` and `master.key` live.
    pub(crate) fn secrets_dir(&self) -> PathBuf {
        self.data_dir.join("secrets")
    }

    /// Where materialization leases (application-owned, temporary,
    /// individually-leased copies of archive entries extracted onto real
    /// disk paths -- see `crate::materialization`) are rooted. Deliberately
    /// under `cache_dir`, not the OS temp directory: unlike the leaked
    /// `std::env::temp_dir()`-rooted directory the pre-facade UI used
    /// (`arclain_<pid>`, shared by every `FileOpener` in one process run),
    /// this application owns the whole subtree, names each lease's own
    /// directory by its `MaterializationLeaseId`, and is responsible for
    /// removing it again -- on release, on expiry, and on shutdown.
    pub(crate) fn materialization_dir(&self) -> PathBuf {
        self.cache_dir.join("materialization")
    }
}

/// Resolves the directory plugin `.wasm` binaries are loaded from.
///
/// Deliberately independent of [`arclain_app_fs::AppDirectories`]'s own
/// `plugins_dir` field (`config_dir/plugins`): that field has never been
/// read anywhere in this codebase outside its own tests -- it predates
/// this resolution algorithm and was never wired up to it. The order
/// here (env override, then an executable-adjacent `plugins/` folder if
/// it exists, then a dev-repo-relative fallback for `cargo run`, then
/// the executable-adjacent path regardless) matches exactly what
/// `crates/ui/src/core/state/init.rs` did before this task moved it.
pub(crate) fn resolve_plugins_dir() -> PathBuf {
    let override_dir = std::env::var_os(PLUGINS_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    resolve_plugins_dir_from(
        override_dir,
        std::env::current_exe().ok(),
        dev_plugins_dir(),
    )
}

fn resolve_plugins_dir_from(
    override_dir: Option<PathBuf>,
    exe_path: Option<PathBuf>,
    dev_plugins_dir: Option<PathBuf>,
) -> PathBuf {
    if let Some(path) = override_dir {
        return path;
    }

    let bundled_plugins_dir = exe_path
        .as_deref()
        .and_then(Path::parent)
        .map(|dir| dir.join("plugins"));

    if let Some(path) = bundled_plugins_dir.as_ref().filter(|path| path.exists()) {
        return path.clone();
    }

    if let Some(path) = dev_plugins_dir.filter(|path| path.exists()) {
        return path;
    }

    bundled_plugins_dir.unwrap_or_else(|| PathBuf::from("plugins"))
}

fn dev_plugins_dir() -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    {
        // `crates/app` and `crates/ui` are both two levels below the
        // repo root (`repo_root/crates/<name>`), so this resolves to
        // the identical `repo_root/plugins` regardless of which crate's
        // `CARGO_MANIFEST_DIR` is compiled in.
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        manifest_dir
            .parent()
            .and_then(Path::parent)
            .map(|repo_root| repo_root.join("plugins"))
    }

    #[cfg(not(debug_assertions))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn plugin_dir_env_override_wins() {
        let temp = tempfile::TempDir::new().unwrap();
        let override_dir = temp.path().join("override-plugins");
        let exe_path = temp.path().join("install").join("arclain.exe");
        let dev_plugins = temp.path().join("repo").join("plugins");

        let resolved = resolve_plugins_dir_from(
            Some(override_dir.clone()),
            Some(exe_path),
            Some(dev_plugins),
        );

        assert_eq!(resolved, override_dir);
    }

    #[test]
    fn executable_adjacent_plugins_dir_wins_when_it_exists() {
        let temp = tempfile::TempDir::new().unwrap();
        let install_dir = temp.path().join("install");
        let bundled_plugins = install_dir.join("plugins");
        let dev_plugins = temp.path().join("repo").join("plugins");
        fs::create_dir_all(&bundled_plugins).unwrap();
        fs::create_dir_all(&dev_plugins).unwrap();

        let resolved = resolve_plugins_dir_from(
            None,
            Some(install_dir.join("arclain.exe")),
            Some(dev_plugins),
        );

        assert_eq!(resolved, bundled_plugins);
    }

    #[test]
    fn dev_plugins_dir_is_used_when_bundled_dir_is_missing() {
        let temp = tempfile::TempDir::new().unwrap();
        let install_dir = temp.path().join("install");
        let dev_plugins = temp.path().join("repo").join("plugins");
        fs::create_dir_all(&dev_plugins).unwrap();

        let resolved = resolve_plugins_dir_from(
            None,
            Some(install_dir.join("arclain.exe")),
            Some(dev_plugins.clone()),
        );

        assert_eq!(resolved, dev_plugins);
    }

    #[test]
    fn system_default_is_pure_and_deterministic() {
        let a = AppPaths::system_default().unwrap();
        let b = AppPaths::system_default().unwrap();
        assert_eq!(a.config_dir, b.config_dir);
        assert_eq!(a.config_dir.file_name().unwrap(), APP_NAME);
        // Today's reality: config and data are the same directory.
        assert_eq!(a.config_dir, a.data_dir);
    }

    #[test]
    fn resolve_uses_override_verbatim_when_given() {
        let temp = tempfile::TempDir::new().unwrap();
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            log_dir: temp.path().join("logs"),
            plugins_dir: temp.path().join("plugins"),
        };
        let resolved = AppPaths::resolve(Some(paths.clone())).unwrap();
        assert_eq!(resolved.config_dir, paths.config_dir);
        assert_eq!(resolved.data_dir, paths.data_dir);
    }

    #[test]
    fn ensure_created_creates_the_databases_and_secrets_subdirectories() {
        let temp = tempfile::TempDir::new().unwrap();
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            log_dir: temp.path().join("logs"),
            plugins_dir: temp.path().join("plugins"),
        };
        paths.ensure_created().unwrap();
        assert!(paths.config_dir.is_dir());
        assert!(paths.databases_dir().is_dir());
        assert!(paths.secrets_dir().is_dir());
    }

    #[test]
    fn ensure_created_is_idempotent() {
        let temp = tempfile::TempDir::new().unwrap();
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            log_dir: temp.path().join("logs"),
            plugins_dir: temp.path().join("plugins"),
        };
        paths.ensure_created().unwrap();
        paths.ensure_created().unwrap();
        assert!(paths.config_dir.is_dir());
    }
}
