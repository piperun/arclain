//! Progress-enabled extraction operations for 7-Zip CLI

use super::{ChildWithProgress, SevenZipCli};
use crate::ArchiveBackend;
use anyhow::Result;
use std::ffi::OsString;
use std::path::Path;

impl SevenZipCli {
    /// Like `extract_files`, but returns a running process with progress updates.
    pub fn spawn_extract_files_with_progress(
        &self,
        path: &Path,
        dest: &Path,
        files: &[String],
        password: Option<&str>,
    ) -> Result<ChildWithProgress> {
        let mut args = vec![
            OsString::from("x"),
            OsString::from("-y"),
            OsString::from("-mmt=on"),
            OsString::from("-sccUTF-8"),
            OsString::from("-scsUTF-8"),
        ];
        if let Some(p) = password {
            args.push(OsString::from(format!("-p{}", p)));
        } else {
            args.push(OsString::from("-p"));
        }
        let mut oarg = OsString::from("-o");
        oarg.push(dest.as_os_str());
        args.push(oarg);
        args.push(path.as_os_str().to_os_string());
        for f in files {
            args.push(OsString::from(f));
        }
        self.spawn_with_progress(args)
    }

    /// Like `extract_all`, but returns a running process with progress updates.
    pub fn spawn_extract_all_with_progress(
        &self,
        path: &Path,
        dest: &Path,
        password: Option<&str>,
    ) -> Result<ChildWithProgress> {
        let mut args = vec![
            OsString::from("x"),
            OsString::from("-y"),
            OsString::from("-mmt=on"),
            OsString::from("-sccUTF-8"),
            OsString::from("-scsUTF-8"),
        ];
        if let Some(p) = password {
            args.push(OsString::from(format!("-p{}", p)));
        } else {
            args.push(OsString::from("-p"));
        }
        let mut oarg = OsString::from("-o");
        oarg.push(dest.as_os_str());
        args.push(oarg);
        args.push(path.as_os_str().to_os_string());
        self.spawn_with_progress(args)
    }

    /// Like `extract_directory`, but returns a running process with progress updates.
    pub fn spawn_extract_directory_with_progress(
        &self,
        path: &Path,
        dest: &Path,
        dir_path: &str,
        password: Option<&str>,
    ) -> Result<ChildWithProgress> {
        let mut args = vec![
            OsString::from("x"),
            OsString::from("-y"),
            OsString::from("-mmt=on"),
            OsString::from("-sccUTF-8"),
            OsString::from("-scsUTF-8"),
        ];
        if let Some(p) = password {
            args.push(OsString::from(format!("-p{}", p)));
        } else {
            args.push(OsString::from("-p"));
        }
        let mut oarg = OsString::from("-o");
        oarg.push(dest.as_os_str());
        args.push(oarg);
        args.push(path.as_os_str().to_os_string());
        if !dir_path.is_empty() {
            let pattern = format!("{}/*", dir_path.trim_end_matches('/'));
            args.push(OsString::from(pattern));
        }
        self.spawn_with_progress(args)
    }

    /// Like `compress`, but returns a running process with progress updates for conversion
    pub fn spawn_convert_with_progress(
        &self,
        source_dir: &Path,
        dest_7z: &Path,
    ) -> Result<ChildWithProgress> {
        let mut args = vec![
            OsString::from("a"),
            OsString::from("-t7z"),
            OsString::from("-mx=5"), // Normal compression (faster than -mx=9)
            OsString::from("-m0=LZMA2"),
            OsString::from("-mmt=on"), // Multi-threaded
            OsString::from("-sccUTF-8"),
            OsString::from("-scsUTF-8"),
            dest_7z.as_os_str().to_os_string(),
        ];

        // Add all files/folders in source directory
        args.push(OsString::from(format!(
            "{}{}*",
            source_dir.display(),
            std::path::MAIN_SEPARATOR
        )));

        self.spawn_with_progress(args)
    }

    /// Extract files with progress callback support
    pub fn extract_files_with_progress(
        &self,
        archive: &Path,
        dest: &Path,
        files: &[String],
        password: Option<&str>,
        progress: Option<&crate::ProgressCallback>,
        _cancel: Option<&crate::CancellationToken>,
    ) -> Result<()> {
        // For now, just delegate to extract_files with progress reporting
        if let Some(cb) = progress {
            cb(crate::ExtractionProgress {
                current: 0,
                total: files.len(),
                current_file: "Starting...".to_string(),
                percent: 0,
            });
        }

        let result = self.extract_files(archive, dest, files, password);

        if let Some(cb) = progress {
            cb(crate::ExtractionProgress {
                current: files.len(),
                total: files.len(),
                current_file: "Complete".to_string(),
                percent: 100,
            });
        }

        result
    }
}
