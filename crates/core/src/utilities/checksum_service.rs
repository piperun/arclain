//! Checksum verification service for file operations
//!
//! Provides high-level API for verifying file integrity during
//! extraction and organization operations.

use anyhow::Result;
use arclain_checksum::{hash_folder_parallel, Algorithm};
use arclain_db::{
    begin_checksum_operation, delete_checksum_operation, get_checksum_algorithm, get_checksum_mode,
    get_merkle_root, get_pending_checksum_operations, store_file_checksum, store_merkle_root,
    update_checksum_operation, ChecksumDb, DbOperation, OpId, OpState, OpType, VerifyMode,
};
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Checksum verification service
pub struct ChecksumService {
    db: ChecksumDb,
    algorithm: Algorithm,
    mode: VerifyMode,
}

impl ChecksumService {
    /// Open or create the checksum service
    pub fn open(db_path: &Path) -> Result<Self> {
        let db = ChecksumDb::open(db_path)?;

        let algo_str = db.with_conn(|conn| get_checksum_algorithm(conn))?;
        let algorithm = Algorithm::from_str(&algo_str).unwrap_or_default();

        let mode = db.with_conn(|conn| get_checksum_mode(conn))?;

        Ok(Self {
            db,
            algorithm,
            mode,
        })
    }

    /// Check if verification is enabled
    pub fn is_enabled(&self) -> bool {
        self.mode != VerifyMode::Disabled
    }

    /// Get current algorithm
    pub fn algorithm(&self) -> Algorithm {
        self.algorithm
    }

    /// Get current mode
    pub fn mode(&self) -> VerifyMode {
        self.mode
    }

    /// Set algorithm
    pub fn set_algorithm(&mut self, algo: Algorithm) -> Result<()> {
        self.db.with_conn(|conn| {
            arclain_db::set_checksum_algorithm(conn, algo.to_string().as_str())
        })?;
        self.algorithm = algo;
        Ok(())
    }

    /// Set verification mode
    pub fn set_mode(&mut self, mode: VerifyMode) -> Result<()> {
        self.db
            .with_conn(|conn| arclain_db::set_checksum_mode(conn, mode))?;
        self.mode = mode;
        Ok(())
    }

    /// Begin tracking an extraction operation
    pub fn begin_extraction(&self, source: &Path, dest: &Path) -> Result<OpId> {
        if !self.is_enabled() {
            return Ok(OpId::new());
        }

        let op = DbOperation {
            id: OpId::new(),
            op_type: OpType::Extract,
            state: OpState::Pending,
            source_path: source.to_path_buf(),
            dest_path: Some(dest.to_path_buf()),
            source_hash: None,
            dest_hash: None,
            error_message: None,
            created_at: now(),
            updated_at: now(),
        };

        self.db
            .with_conn(|conn| begin_checksum_operation(conn, &op))?;
        debug!("Started extraction operation {:?}", op.id);
        Ok(op.id)
    }

    /// Verify extraction result
    pub fn verify_extraction(&self, op_id: &OpId, dest_folder: &Path) -> Result<VerifyResult> {
        if !self.is_enabled() {
            return Ok(VerifyResult::Skipped);
        }

        info!("Verifying extraction at {}", dest_folder.display());

        // Hash the extracted folder
        let (file_results, merkle) = hash_folder_parallel(dest_folder, self.algorithm, None)?;

        // Store results based on mode
        match self.mode {
            VerifyMode::Simple => {
                // Just store root hash
                let archive_id = dest_folder.to_string_lossy().to_string();
                self.db.with_conn(|conn| {
                    store_merkle_root(
                        conn,
                        &archive_id,
                        &merkle.root_hash().bytes,
                        merkle.file_count(),
                        &self.algorithm.to_string(),
                    )
                })?;
            }
            VerifyMode::Full => {
                // Store all file checksums
                let archive_id = dest_folder.to_string_lossy().to_string();
                for file in &file_results {
                    self.db.with_conn(|conn| {
                        store_file_checksum(
                            conn,
                            &file.relative_path,
                            Some(&archive_id),
                            &file.hash.bytes,
                            file.size,
                            &self.algorithm.to_string(),
                        )
                    })?;
                }
                // Also store root
                self.db.with_conn(|conn| {
                    store_merkle_root(
                        conn,
                        &archive_id,
                        &merkle.root_hash().bytes,
                        merkle.file_count(),
                        &self.algorithm.to_string(),
                    )
                })?;
            }
            VerifyMode::Disabled => {}
        }

        // Mark operation complete
        self.db.with_conn(|conn| {
            let op = DbOperation {
                id: op_id.clone(),
                op_type: OpType::Extract,
                state: OpState::Completed,
                source_path: PathBuf::new(),
                dest_path: Some(dest_folder.to_path_buf()),
                source_hash: None,
                dest_hash: Some(merkle.root_hash().bytes.clone()),
                error_message: None,
                created_at: now(),
                updated_at: now(),
            };
            update_checksum_operation(conn, &op)
        })?;

        info!(
            "Extraction verified: {} files, root hash: {}",
            merkle.file_count(),
            merkle.root_hash().to_hex()
        );

        Ok(VerifyResult::Verified {
            file_count: merkle.file_count(),
            root_hash: merkle.root_hash().to_hex(),
        })
    }

    /// Verify a folder against stored checksum
    pub fn verify_folder(&self, folder: &Path) -> Result<VerifyResult> {
        if !self.is_enabled() {
            return Ok(VerifyResult::Skipped);
        }

        let archive_id = folder.to_string_lossy().to_string();

        // Get stored root hash
        let stored_root = self
            .db
            .with_conn(|conn| get_merkle_root(conn, &archive_id))?;

        let stored_root = match stored_root {
            Some(h) => h,
            None => {
                return Ok(VerifyResult::NoChecksum);
            }
        };

        // Compute current hash
        let (_, merkle) = hash_folder_parallel(folder, self.algorithm, None)?;

        if merkle.root_hash().bytes == stored_root {
            Ok(VerifyResult::Match)
        } else {
            Ok(VerifyResult::Mismatch {
                expected: hex_encode(&stored_root),
                actual: merkle.root_hash().to_hex(),
            })
        }
    }

    /// Begin tracking an organize operation
    pub fn begin_organize(&self, source: &Path, dest: &Path) -> Result<OpId> {
        if !self.is_enabled() {
            return Ok(OpId::new());
        }

        let op = DbOperation {
            id: OpId::new(),
            op_type: OpType::Organize,
            state: OpState::Pending,
            source_path: source.to_path_buf(),
            dest_path: Some(dest.to_path_buf()),
            source_hash: None,
            dest_hash: None,
            error_message: None,
            created_at: now(),
            updated_at: now(),
        };

        self.db
            .with_conn(|conn| begin_checksum_operation(conn, &op))?;
        debug!("Started organize operation {:?}", op.id);
        Ok(op.id)
    }

    /// Complete an organize operation with verification
    pub fn complete_organize(&self, op_id: &OpId, dest: &Path) -> Result<VerifyResult> {
        self.verify_extraction(op_id, dest)
    }

    /// Recover interrupted operations on startup
    pub fn recover_pending(&self) -> Result<Vec<RecoveryAction>> {
        let pending = self
            .db
            .with_conn(|conn| get_pending_checksum_operations(conn))?;

        let mut actions = Vec::new();

        for op in pending {
            let action = match op.state {
                OpState::Pending => {
                    // Never started, just clean up
                    self.db
                        .with_conn(|conn| delete_checksum_operation(conn, &op.id))?;
                    RecoveryAction::Cleaned(op.id.clone())
                }
                OpState::SourceHashed | OpState::Copied => {
                    // Check if destination exists and re-verify
                    if let Some(ref dest) = op.dest_path {
                        if dest.exists() {
                            match self.verify_folder(dest) {
                                Ok(VerifyResult::Match) | Ok(VerifyResult::Verified { .. }) => {
                                    self.db.with_conn(|conn| {
                                        let mut completed = op.clone();
                                        completed.state = OpState::Completed;
                                        completed.updated_at = now();
                                        update_checksum_operation(conn, &completed)
                                    })?;
                                    RecoveryAction::Completed(op.id.clone())
                                }
                                Ok(VerifyResult::Mismatch { expected, actual }) => {
                                    RecoveryAction::Failed {
                                        op_id: op.id.clone(),
                                        reason: format!(
                                            "Hash mismatch: expected {}, got {}",
                                            expected, actual
                                        ),
                                    }
                                }
                                _ => {
                                    self.db.with_conn(|conn| {
                                        delete_checksum_operation(conn, &op.id)
                                    })?;
                                    RecoveryAction::Cleaned(op.id.clone())
                                }
                            }
                        } else {
                            // Destination doesn't exist, clean up
                            self.db
                                .with_conn(|conn| delete_checksum_operation(conn, &op.id))?;
                            RecoveryAction::Cleaned(op.id.clone())
                        }
                    } else {
                        self.db
                            .with_conn(|conn| delete_checksum_operation(conn, &op.id))?;
                        RecoveryAction::Cleaned(op.id.clone())
                    }
                }
                OpState::DestVerified => {
                    // Just finalize
                    self.db.with_conn(|conn| {
                        let mut completed = op.clone();
                        completed.state = OpState::Completed;
                        completed.updated_at = now();
                        update_checksum_operation(conn, &completed)
                    })?;
                    RecoveryAction::Completed(op.id.clone())
                }
                OpState::Completed | OpState::Failed => {
                    // Already done
                    RecoveryAction::AlreadyDone(op.id.clone())
                }
            };

            actions.push(action);
        }

        if !actions.is_empty() {
            info!("Recovered {} pending checksum operations", actions.len());
        }

        Ok(actions)
    }
}

/// Result of a verification operation
#[derive(Debug, Clone)]
pub enum VerifyResult {
    /// Verification was skipped (disabled)
    Skipped,
    /// No stored checksum to compare against
    NoChecksum,
    /// Verification passed
    Verified {
        file_count: usize,
        root_hash: String,
    },
    /// Stored checksum matches
    Match,
    /// Checksum mismatch
    Mismatch { expected: String, actual: String },
}

/// Recovery action taken for a pending operation
#[derive(Debug, Clone)]
pub enum RecoveryAction {
    /// Operation was cleaned up (never started properly)
    Cleaned(OpId),
    /// Operation was completed after re-verification
    Completed(OpId),
    /// Operation was already done
    AlreadyDone(OpId),
    /// Operation failed verification
    Failed { op_id: OpId, reason: String },
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}
