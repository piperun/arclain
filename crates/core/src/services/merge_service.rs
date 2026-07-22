//! Service for merging multi-part archives into single archives

use crate::archive::{CancellationToken, MultiPartArchive};
use crate::backends::selector::BackendSelector;
use crate::backends::sevenz_cli::SevenZipCli;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use tempfile::TempDir;
use tracing::{debug, info, warn};

/// Output format for merged archives
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    SevenZip,
    Zip,
}

impl OutputFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            Self::SevenZip => "7z",
            Self::Zip => "zip",
        }
    }

    pub fn format_arg(&self) -> &'static str {
        match self {
            Self::SevenZip => "7z",
            Self::Zip => "zip",
        }
    }

    pub fn all() -> &'static [OutputFormat] {
        &[OutputFormat::SevenZip, OutputFormat::Zip]
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::SevenZip => "7-Zip (.7z)",
            Self::Zip => "ZIP (.zip)",
        }
    }
}

/// Compression level for output archives
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompressionLevel {
    Store,
    Fastest,
    Fast,
    #[default]
    Normal,
    Maximum,
    Ultra,
}

impl CompressionLevel {
    pub fn to_7z_arg(&self) -> &'static str {
        match self {
            Self::Store => "-mx=0",
            Self::Fastest => "-mx=1",
            Self::Fast => "-mx=3",
            Self::Normal => "-mx=5",
            Self::Maximum => "-mx=7",
            Self::Ultra => "-mx=9",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Store => "Store (no compression)",
            Self::Fastest => "Fastest",
            Self::Fast => "Fast",
            Self::Normal => "Normal",
            Self::Maximum => "Maximum",
            Self::Ultra => "Ultra",
        }
    }

    pub fn all() -> &'static [CompressionLevel] {
        &[
            CompressionLevel::Store,
            CompressionLevel::Fastest,
            CompressionLevel::Fast,
            CompressionLevel::Normal,
            CompressionLevel::Maximum,
            CompressionLevel::Ultra,
        ]
    }
}

/// Options for merging multi-part archives
#[derive(Debug, Clone)]
pub struct MergeOptions {
    /// Output format
    pub output_format: OutputFormat,
    /// Output path (if None, uses same directory as source with new extension)
    pub output_path: Option<PathBuf>,
    /// Compression level
    pub compression_level: CompressionLevel,
    /// Whether to delete original parts after successful merge
    pub delete_originals: bool,
    /// Optional password for encrypted source archives
    pub password: Option<String>,
}

impl Default for MergeOptions {
    fn default() -> Self {
        Self {
            output_format: OutputFormat::SevenZip,
            output_path: None,
            compression_level: CompressionLevel::Normal,
            delete_originals: false,
            password: None,
        }
    }
}

/// Progress callback for merge operations
pub type MergeProgressCallback = Box<dyn Fn(MergeProgress) + Send + Sync>;

/// Progress update for merge operations
#[derive(Debug, Clone)]
pub struct MergeProgress {
    pub phase: MergePhase,
    pub percent: u8,
    pub message: String,
}

/// Phase of the merge operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergePhase {
    Validating,
    Extracting,
    Compressing,
    Cleaning,
    Complete,
    Failed,
}

impl MergePhase {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Validating => "Validating parts",
            Self::Extracting => "Extracting",
            Self::Compressing => "Compressing",
            Self::Cleaning => "Cleaning up",
            Self::Complete => "Complete",
            Self::Failed => "Failed",
        }
    }
}

/// Service for merging multi-part archives
pub struct MergeService {
    backend_selector: BackendSelector,
}

impl MergeService {
    pub fn new(backend_selector: BackendSelector) -> Self {
        Self { backend_selector }
    }

    pub fn with_default_selector() -> Self {
        Self::new(BackendSelector::default())
    }

    /// Merge a multi-part archive into a single archive
    pub fn merge(
        &self,
        multipart: &mut MultiPartArchive,
        options: MergeOptions,
        progress: Option<MergeProgressCallback>,
        cancel: Option<CancellationToken>,
    ) -> Result<PathBuf> {
        let report_progress = |phase: MergePhase, percent: u8, message: &str| {
            if let Some(ref cb) = progress {
                cb(MergeProgress {
                    phase,
                    percent,
                    message: message.to_string(),
                });
            }
        };

        let check_cancelled = || -> Result<()> {
            if let Some(ref token) = cancel {
                if token.load(Ordering::Relaxed) {
                    anyhow::bail!("Operation cancelled");
                }
            }
            Ok(())
        };

        // Phase 1: Validate
        report_progress(MergePhase::Validating, 0, "Validating archive parts...");
        let validation = multipart.validate()?;

        if !validation.is_complete {
            anyhow::bail!(
                "Missing archive parts: {:?}",
                validation.missing_parts.join(", ")
            );
        }

        let part_count = validation.found_parts.len();
        info!(
            "Validated {} parts, total size: {} bytes",
            part_count, validation.total_size
        );

        check_cancelled()?;

        // Determine output path
        let output_path = options.output_path.clone().unwrap_or_else(|| {
            let parent = multipart
                .first_part
                .parent()
                .unwrap_or_else(|| Path::new("."));
            parent.join(format!(
                "{}.{}",
                multipart.base_name,
                options.output_format.extension()
            ))
        });

        // Check if output already exists
        if output_path.exists() {
            anyhow::bail!("Output file already exists: {}", output_path.display());
        }

        // Phase 2: Extract to temp directory
        report_progress(
            MergePhase::Extracting,
            10,
            "Creating temporary directory...",
        );
        let temp_dir = TempDir::new().context("Failed to create temp directory")?;
        let temp_path = temp_dir.path();

        debug!("Extracting to temp directory: {}", temp_path.display());

        // Select backend based on the first part
        let backend = self.backend_selector.select(&multipart.first_part)?;

        report_progress(
            MergePhase::Extracting,
            15,
            &format!("Extracting {} parts...", part_count),
        );

        // Extract the first part (7z/unrar will automatically process all parts)
        backend.extract_all(
            &multipart.first_part,
            temp_path,
            options.password.as_deref(),
        )?;

        check_cancelled()?;

        // Phase 3: Compress to output format
        report_progress(
            MergePhase::Compressing,
            60,
            "Compressing to output format...",
        );

        // Get list of files to compress
        let files_to_compress: Vec<PathBuf> = walkdir::WalkDir::new(temp_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| e.path().to_path_buf())
            .collect();

        if files_to_compress.is_empty() {
            anyhow::bail!("No files were extracted from the archive");
        }

        info!(
            "Compressing {} files to {}",
            files_to_compress.len(),
            output_path.display()
        );

        // Use 7z CLI for compression (it supports both 7z and zip output)
        let sevenz_cli = SevenZipCli::detect(None)?;

        // Create archive with compression settings
        self.create_archive_with_options(&sevenz_cli, &output_path, temp_path, &options)?;

        check_cancelled()?;

        // Phase 4: Clean up originals if requested
        if options.delete_originals {
            report_progress(MergePhase::Cleaning, 90, "Deleting original parts...");

            for part in &validation.found_parts {
                if let Err(e) = std::fs::remove_file(part) {
                    warn!("Failed to delete {}: {}", part.display(), e);
                } else {
                    debug!("Deleted {}", part.display());
                }
            }
        }

        report_progress(MergePhase::Complete, 100, "Merge complete");
        info!("Successfully merged to {}", output_path.display());

        Ok(output_path)
    }

    fn create_archive_with_options(
        &self,
        sevenz: &SevenZipCli,
        output: &Path,
        source_dir: &Path,
        options: &MergeOptions,
    ) -> Result<()> {
        use std::process::Command;

        let sevenz_path = sevenz.exe_path();

        let mut cmd = Command::new(sevenz_path);
        cmd.arg("a"); // Add to archive

        // Output format
        cmd.arg(format!("-t{}", options.output_format.format_arg()));

        // Compression level
        cmd.arg(options.compression_level.to_7z_arg());

        // For 7z, use LZMA2
        if options.output_format == OutputFormat::SevenZip {
            cmd.arg("-m0=LZMA2");
        }

        // Output file
        cmd.arg(output);

        // Source directory (with wildcard to include all contents)
        let source_pattern = source_dir.join("*");
        cmd.arg(&source_pattern);

        // Recurse subdirectories
        cmd.arg("-r");

        // Suppress prompts
        cmd.arg("-y");

        crate::utilities::hide_console(&mut cmd);

        debug!("Running: {:?}", cmd);

        let output = cmd
            .output()
            .context("Failed to execute 7z compression command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            anyhow::bail!(
                "7z compression failed:\nstdout: {}\nstderr: {}",
                stdout,
                stderr
            );
        }

        Ok(())
    }

    /// Get a preview of what the merge operation will do
    pub fn preview_merge(
        &self,
        multipart: &mut MultiPartArchive,
        options: &MergeOptions,
    ) -> Result<MergePreview> {
        let validation = multipart.validate()?;

        let output_path = options.output_path.clone().unwrap_or_else(|| {
            let parent = multipart
                .first_part
                .parent()
                .unwrap_or_else(|| Path::new("."));
            parent.join(format!(
                "{}.{}",
                multipart.base_name,
                options.output_format.extension()
            ))
        });

        Ok(MergePreview {
            source_parts: validation.found_parts,
            source_total_size: validation.total_size,
            output_path,
            output_format: options.output_format,
            will_delete_originals: options.delete_originals,
            is_valid: validation.is_complete,
            missing_parts: validation.missing_parts,
        })
    }
}

/// Preview of a merge operation
#[derive(Debug, Clone)]
pub struct MergePreview {
    pub source_parts: Vec<PathBuf>,
    pub source_total_size: u64,
    pub output_path: PathBuf,
    pub output_format: OutputFormat,
    pub will_delete_originals: bool,
    pub is_valid: bool,
    pub missing_parts: Vec<String>,
}

impl MergePreview {
    pub fn part_count(&self) -> usize {
        self.source_parts.len()
    }

    pub fn formatted_size(&self) -> String {
        format_size(self.source_total_size)
    }
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}
