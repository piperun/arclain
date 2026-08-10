use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::OpenOptions;
use parking_lot::Mutex;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use wirt::{PackageFingerprint, PluginError, Result, TrustedPluginRoot};

const LEDGER_FILE: &str = ".wirt-quarantine.json";
const LEDGER_VERSION: u8 = 1;
const MAX_LEDGER_BYTES: usize = 1024 * 1024;
const MAX_LEDGER_RECORDS: usize = 1024;
const MAX_REASON_BYTES: usize = 256;
const MAX_FAILED_RETRIES: u8 = 3;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QuarantineRecord {
    pub failed_retries: u8,
    pub last_reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QuarantineState {
    Clear,
    Retryable(QuarantineRecord),
    PersistentlyDisabled(QuarantineRecord),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LedgerFile {
    version: u8,
    #[serde(deserialize_with = "deserialize_records")]
    records: BTreeMap<String, QuarantineRecord>,
}

fn deserialize_records<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, QuarantineRecord>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct RecordsVisitor;

    impl<'de> Visitor<'de> for RecordsVisitor {
        type Value = BTreeMap<String, QuarantineRecord>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a quarantine record map with unique fingerprints")
        }

        fn visit_map<A>(self, mut entries: A) -> std::result::Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut records = BTreeMap::new();
            while let Some((fingerprint, record)) = entries.next_entry()? {
                if records.insert(fingerprint, record).is_some() {
                    return Err(serde::de::Error::custom("duplicate quarantine fingerprint"));
                }
            }
            Ok(records)
        }
    }

    deserializer.deserialize_map(RecordsVisitor)
}

#[derive(Default)]
struct LedgerState {
    persisted: BTreeMap<String, QuarantineRecord>,
    runtime_violations: BTreeMap<String, String>,
}

pub struct QuarantineLedger {
    root: Arc<TrustedPluginRoot>,
    state: Mutex<LedgerState>,
}

impl std::fmt::Debug for QuarantineLedger {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QuarantineLedger")
            .field("records", &self.state.lock().persisted.len())
            .finish_non_exhaustive()
    }
}

impl QuarantineLedger {
    pub fn open(root: Arc<TrustedPluginRoot>) -> Result<Self> {
        root.revalidate_current_path()
            .map_err(|_| ledger_io_error("opened"))?;
        let persisted = read_ledger(&root)?;
        Ok(Self {
            root,
            state: Mutex::new(LedgerState {
                persisted,
                runtime_violations: BTreeMap::new(),
            }),
        })
    }

    pub fn state(&self, fingerprint: &PackageFingerprint) -> QuarantineState {
        let state = self.state.lock();
        let key = fingerprint.as_str();
        let persisted = state.persisted.get(key);
        let runtime_reason = state.runtime_violations.get(key);
        let record = match (persisted, runtime_reason) {
            (Some(record), Some(reason)) => QuarantineRecord {
                failed_retries: record.failed_retries,
                last_reason: reason.clone(),
            },
            (Some(record), None) => record.clone(),
            (None, Some(reason)) => QuarantineRecord {
                failed_retries: 0,
                last_reason: reason.clone(),
            },
            (None, None) => return QuarantineState::Clear,
        };
        if record.failed_retries >= MAX_FAILED_RETRIES {
            QuarantineState::PersistentlyDisabled(record)
        } else {
            QuarantineState::Retryable(record)
        }
    }

    pub fn record_initial_violation(
        &self,
        fingerprint: &PackageFingerprint,
        reason: &str,
    ) -> Result<()> {
        validate_reason(reason)?;
        self.state
            .lock()
            .runtime_violations
            .insert(fingerprint.to_string(), reason.to_owned());
        Ok(())
    }

    pub fn record_failed_retry(
        &self,
        fingerprint: &PackageFingerprint,
        reason: &str,
    ) -> Result<QuarantineRecord> {
        validate_reason(reason)?;
        let mut state = self.state.lock();
        let key = fingerprint.to_string();
        let mut next = state.persisted.clone();
        let failed_retries = next
            .get(&key)
            .map_or(1, |record| record.failed_retries.saturating_add(1))
            .min(MAX_FAILED_RETRIES);
        let record = QuarantineRecord {
            failed_retries,
            last_reason: reason.to_owned(),
        };
        next.insert(key.clone(), record.clone());
        write_ledger(&self.root, &next)?;
        state.persisted = next;
        state.runtime_violations.insert(key, reason.to_owned());
        Ok(record)
    }

    pub fn reset(&self, fingerprint: &PackageFingerprint) -> Result<()> {
        let mut state = self.state.lock();
        let mut next = state.persisted.clone();
        next.remove(fingerprint.as_str());
        write_ledger(&self.root, &next)?;
        state.persisted = next;
        state.runtime_violations.remove(fingerprint.as_str());
        Ok(())
    }

    pub fn clear_runtime_violation(&self, fingerprint: &PackageFingerprint) {
        self.state
            .lock()
            .runtime_violations
            .remove(fingerprint.as_str());
    }

    pub fn has_runtime_violation(&self, fingerprint: &PackageFingerprint) -> bool {
        self.state
            .lock()
            .runtime_violations
            .contains_key(fingerprint.as_str())
    }
}

fn validate_reason(reason: &str) -> Result<()> {
    if reason.is_empty() || reason.len() > MAX_REASON_BYTES {
        return Err(invalid_ledger());
    }
    Ok(())
}

fn validate_records(records: &BTreeMap<String, QuarantineRecord>) -> Result<()> {
    if records.len() > MAX_LEDGER_RECORDS {
        return Err(invalid_ledger());
    }
    for (fingerprint, record) in records {
        fingerprint
            .parse::<PackageFingerprint>()
            .map_err(|_| invalid_ledger())?;
        if !(1..=MAX_FAILED_RETRIES).contains(&record.failed_retries) {
            return Err(invalid_ledger());
        }
        validate_reason(&record.last_reason)?;
    }
    Ok(())
}

fn read_ledger(root: &TrustedPluginRoot) -> Result<BTreeMap<String, QuarantineRecord>> {
    let metadata = match root.dir().symlink_metadata(LEDGER_FILE) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(_) => return Err(ledger_io_error("inspected")),
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_LEDGER_BYTES as u64
    {
        return Err(invalid_ledger());
    }

    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let file = root
        .dir()
        .open_with(LEDGER_FILE, &options)
        .map_err(|_| ledger_io_error("opened"))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_LEDGER_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ledger_io_error("read"))?;
    if bytes.len() > MAX_LEDGER_BYTES {
        return Err(invalid_ledger());
    }
    let ledger: LedgerFile = serde_json::from_slice(&bytes).map_err(|_| invalid_ledger())?;
    if ledger.version != LEDGER_VERSION {
        return Err(invalid_ledger());
    }
    validate_records(&ledger.records)?;
    Ok(ledger.records)
}

fn write_ledger(
    root: &TrustedPluginRoot,
    records: &BTreeMap<String, QuarantineRecord>,
) -> Result<()> {
    validate_records(records)?;
    let bytes = serde_json::to_vec(&LedgerFile {
        version: LEDGER_VERSION,
        records: records.clone(),
    })
    .map_err(|_| invalid_ledger())?;
    if bytes.len() > MAX_LEDGER_BYTES {
        return Err(invalid_ledger());
    }
    root.revalidate_current_path()
        .map_err(|_| ledger_io_error("written"))?;

    let temp_name = create_temp_name();
    let result = (|| {
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        let mut file = root
            .dir()
            .open_with(&temp_name, &options)
            .map_err(|_| ledger_io_error("created"))?;
        file.write_all(&bytes)
            .map_err(|_| ledger_io_error("written"))?;
        file.sync_all().map_err(|_| ledger_io_error("flushed"))?;
        drop(file);
        root.revalidate_current_path()
            .map_err(|_| ledger_io_error("replaced"))?;
        root.dir()
            .rename(&temp_name, root.dir(), LEDGER_FILE)
            .map_err(|_| ledger_io_error("replaced"))
    })();
    if result.is_err() {
        let _ = root.dir().remove_file(&temp_name);
    }
    result
}

fn create_temp_name() -> String {
    format!(
        "{LEDGER_FILE}.tmp-{}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn invalid_ledger() -> PluginError {
    PluginError::LoadError("plugin quarantine ledger is invalid".to_string())
}

fn ledger_io_error(action: &str) -> PluginError {
    PluginError::LoadError(format!("plugin quarantine ledger could not be {action}"))
}
