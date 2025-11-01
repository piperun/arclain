use anyhow::Result;
use archust_core::sevenzip::SevenZipCli;
use archust_core::{ArchiveBackend, ConfigStore, NavigationState};
use std::path::{Path, PathBuf};
use tracing::{debug, info};

pub struct AppState {
    pub cfg: ConfigStore,
    pub backend: SevenZipCli,
    pub last_entries: Vec<String>,
    pub all_entries: Vec<archust_core::ArchiveEntry>,
    pub navigation: NavigationState,
    pub current_archive: Option<PathBuf>,
    pub archive_encrypted: bool,
    pub headers_encrypted: bool,
    pub encryption_method: Option<String>,
    pub current_password: Option<String>,
}

impl AppState {
    pub fn new() -> Result<Self> {
        info!("Initializing application state");
        let cfg = ConfigStore::load("archust")?;
        let backend = SevenZipCli::detect(cfg.cfg.sevenzip_path.as_deref())?;
        info!("7-Zip backend initialized");
        Ok(Self {
            cfg,
            backend,
            last_entries: vec![],
            all_entries: vec![],
            navigation: NavigationState::new(),
            current_archive: None,
            archive_encrypted: false,
            headers_encrypted: false,
            encryption_method: None,
            current_password: None,
        })
    }

    pub fn list_archive(&mut self, path: &Path) -> Result<Vec<archust_core::ArchiveEntry>> {
        info!("Opening archive: {}", path.display());
        let info = match self.backend.list(path, None) {
            Ok(info) => {
                debug!("Archive opened without password (may have encrypted files inside)");
                info
            }
            Err(e) => {
                debug!("Initial listing failed, trying with auto-password: {}", e);
                let pw = self.cfg.auto_password_for(&self.last_entries);
                if let Some(ref password) = pw {
                    info!("Attempting to open archive with auto-detected password");
                    let info = self.backend.list(path, Some(password))?;
                    self.current_password = Some(password.clone());
                    info
                } else {
                    debug!("No auto-password found");
                    return Err(e);
                }
            }
        };
        self.last_entries = info.entries.iter().map(|e| e.path.clone()).collect();
        if self.current_password.is_none() {
            let detected_pw = self.cfg.auto_password_for(&self.last_entries);
            if let Some(pwd) = detected_pw {
                self.current_password = Some(pwd);
            }
        }
        self.all_entries = info.entries.clone();
        self.current_archive = Some(path.to_path_buf());
        self.archive_encrypted = info.encrypted;
        self.headers_encrypted = info.headers_encrypted;
        self.encryption_method = info.encryption_method.clone();
        self.navigation = NavigationState::new();
        info!(
            "Archive opened successfully with {} entries",
            self.all_entries.len()
        );
        Ok(self.all_entries.clone())
    }

    pub fn list_with_password(
        &mut self,
        path: &Path,
        password: &str,
    ) -> Result<Vec<archust_core::ArchiveEntry>> {
        info!("Listing archive with manually provided password");
        let info = self.backend.list(path, Some(password))?;
        self.last_entries = info.entries.iter().map(|e| e.path.clone()).collect();
        self.all_entries = info.entries.clone();
        self.current_archive = Some(path.to_path_buf());
        self.archive_encrypted = info.encrypted;
        self.headers_encrypted = info.headers_encrypted;
        self.encryption_method = info.encryption_method.clone();
        self.navigation = NavigationState::new();
        self.current_password = Some(password.to_string());
        Ok(self.all_entries.clone())
    }

    pub fn navigate_to_folder(&mut self, folder: &str) {
        debug!("Navigating to folder: {}", folder);
        self.navigation.navigate_to(folder);
    }

    pub fn navigate_back(&mut self) {
        debug!("Navigating back from: {}", self.navigation.current_path);
        self.navigation.navigate_back();
    }

    pub fn navigate_forward(&mut self) {
        debug!("Navigating forward from: {}", self.navigation.current_path);
        self.navigation.navigate_forward();
    }

    pub fn navigate_up(&mut self) {
        debug!("Navigating up from: {}", self.navigation.current_path);
        self.navigation.navigate_up();
    }

    pub fn get_current_entries(&self) -> Vec<archust_core::ArchiveEntry> {
        self.navigation.filter_entries(&self.all_entries)
    }

    pub fn extract_all(&self, archive: &Path, dest: &Path) -> Result<()> {
        info!(
            "Extracting all files from {} to {}",
            archive.display(),
            dest.display()
        );
        let auto_pw = self.cfg.auto_password_for(&self.last_entries);
        let pw = self.current_password.as_deref().or(auto_pw.as_deref());
        self.backend.extract_all(archive, dest, pw)
    }

    pub fn extract_selected(&self, archive: &Path, dest: &Path, files: Vec<String>) -> Result<()> {
        info!("Extracting {} selected files", files.len());
        let auto_pw = self.cfg.auto_password_for(&self.last_entries);
        let pw = self.current_password.as_deref().or(auto_pw.as_deref());

        let full_paths: Vec<String> = if !self.navigation.current_path.is_empty() {
            files
                .iter()
                .map(|f| format!("{}/{}", self.navigation.current_path, f))
                .collect()
        } else {
            files
        };

        self.backend.extract_files(archive, dest, &full_paths, pw)
    }

    pub fn extract_specific(&self, archive: &Path, dest: &Path, full_paths: Vec<String>) -> Result<()> {
        info!("Extracting {} file(s) (exact paths)", full_paths.len());
        let auto_pw = self.cfg.auto_password_for(&self.last_entries);
        let pw = self.current_password.as_deref().or(auto_pw.as_deref());
        if full_paths.len() > 100 {
            let dir_path = if let Some(first) = full_paths.first() {
                if let Some(idx) = first.rfind('/') {
                    &first[..idx]
                } else {
                    ""
                }
            } else {
                ""
            };
            info!(
                "Too many files ({}), extracting entire directory: {}",
                full_paths.len(),
                if dir_path.is_empty() { "<root>" } else { dir_path }
            );
            self.backend.extract_directory(archive, dest, dir_path, pw)
        } else {
            debug!("Files to extract: {:?}", full_paths);
            self.backend.extract_files(archive, dest, &full_paths, pw)
        }
    }

    pub fn add_files_to_archive(&self, archive: &Path, files: Vec<PathBuf>) -> Result<()> {
        self.backend.add_files(archive, &files)
    }

    pub fn read_text_file(&self, archive: &Path, path_in_archive: &str) -> Result<String> {
        let auto_pw = self.cfg.auto_password_for(&self.last_entries);
        let pw = self.current_password.as_deref().or(auto_pw.as_deref());
        self.backend.read_text_file(archive, path_in_archive, pw)
    }

    pub fn delete_files(&self, archive: &Path, files: &[String]) -> Result<()> {
        self.backend.delete_files(archive, files)
    }

    pub fn add_or_update_file_from_str(
        &self,
        archive: &Path,
        path_in_archive: &str,
        content: &str,
    ) -> Result<()> {
        self.backend.add_or_update_file_from_str(archive, path_in_archive, content)
    }
}