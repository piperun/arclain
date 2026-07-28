use super::key_lock::{cache_quota_lock, cache_root_lock};
use crate::traits::CacheIndex;
use anyhow::{bail, Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use ssri::Integrity;
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

const SCOPED_KEY_PREFIX: &str = "\u{1}arclain-cache:v1:";
const MAX_RESERVATION_RECORD_BYTES: u64 = 16 * 1024;
const MAX_RESERVATION_KEY_BYTES: usize = 4 * 1024;
const MAX_OWNER_ID_BYTES: usize = 512;
const INDEX_PAGE_SIZE: usize = 1024;

/// Trust boundary used for cache keys, partial downloads, and quota accounting.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CacheOwner {
    Host,
    Plugin(String),
}

impl CacheOwner {
    pub fn host() -> Self {
        Self::Host
    }

    pub fn plugin(plugin_id: impl Into<String>) -> Self {
        Self::Plugin(plugin_id.into())
    }

    /// Encode an owner and caller key without delimiter ambiguity.
    pub fn scoped_key(&self, key: &str) -> String {
        match self {
            Self::Host => format!("{SCOPED_KEY_PREFIX}h:0::{key}"),
            Self::Plugin(plugin_id) => {
                format!("{SCOPED_KEY_PREFIX}p:{}:{plugin_id}:{key}", plugin_id.len())
            }
        }
    }

    pub fn from_scoped_key(key: &str) -> Option<Self> {
        parse_scoped_key(key).map(|(owner, _)| owner)
    }
}

pub(crate) fn parse_scoped_key(key: &str) -> Option<(CacheOwner, &str)> {
    let encoded = key.strip_prefix(SCOPED_KEY_PREFIX)?;
    if let Some(raw_key) = encoded.strip_prefix("h:0::") {
        return Some((CacheOwner::Host, raw_key));
    }

    let encoded = encoded.strip_prefix("p:")?;
    let length_end = encoded.find(':')?;
    let owner_len: usize = encoded[..length_end].parse().ok()?;
    let owner_and_key = &encoded[length_end + 1..];
    if owner_and_key.len() <= owner_len || owner_and_key.as_bytes().get(owner_len) != Some(&b':') {
        return None;
    }
    if !owner_and_key.is_char_boundary(owner_len) {
        return None;
    }
    let owner = &owner_and_key[..owner_len];
    let raw_key = &owner_and_key[owner_len + 1..];
    Some((CacheOwner::Plugin(owner.to_string()), raw_key))
}

/// Disk-containment limits for cache writes and resumable downloads.
#[derive(Debug, Clone)]
pub struct CacheLimits {
    pub max_object_bytes: u64,
    pub max_owner_partial_bytes: u64,
    pub max_owner_partial_objects: usize,
    pub max_owner_committed_bytes: u64,
    pub max_owner_committed_objects: usize,
    pub max_global_bytes: u64,
    pub max_global_partial_objects: usize,
    pub max_global_committed_objects: usize,
    pub max_queued_writes: usize,
    pub max_queued_bytes: u64,
    pub reservation_chunk_bytes: u64,
    pub min_free_space_bytes: u64,
    pub partial_ttl: Duration,
}

impl Default for CacheLimits {
    fn default() -> Self {
        Self {
            // Chobit video downloads are commonly around 1 GiB. Keep the
            // streaming path useful while bounding a single hostile object.
            max_object_bytes: 4 * 1024 * 1024 * 1024,
            max_owner_partial_bytes: 8 * 1024 * 1024 * 1024,
            max_owner_partial_objects: 64,
            max_owner_committed_bytes: 16 * 1024 * 1024 * 1024,
            max_owner_committed_objects: 20_000,
            max_global_bytes: 64 * 1024 * 1024 * 1024,
            max_global_partial_objects: 256,
            max_global_committed_objects: 100_000,
            max_queued_writes: 32,
            max_queued_bytes: 256 * 1024 * 1024,
            reservation_chunk_bytes: 1024 * 1024,
            min_free_space_bytes: 2 * 1024 * 1024 * 1024,
            partial_ttl: Duration::from_secs(24 * 60 * 60),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReservationRecord {
    owner: CacheOwner,
    scoped_key: String,
    reserved_bytes: u64,
    updated_at_secs: u64,
}

#[derive(Clone)]
struct PendingCommit {
    owner: CacheOwner,
    scoped_key: String,
    bytes: u64,
    objects: usize,
}

#[derive(Default)]
struct PendingCommitRegistry {
    next_id: usize,
    by_root: HashMap<PathBuf, HashMap<usize, PendingCommit>>,
}

static PENDING_COMMITS: OnceLock<Mutex<PendingCommitRegistry>> = OnceLock::new();

fn pending_commit_registry() -> &'static Mutex<PendingCommitRegistry> {
    PENDING_COMMITS.get_or_init(|| Mutex::new(PendingCommitRegistry::default()))
}

/// In-flight committed quota held from admission through the physical and
/// index commit. Early returns and unwinding release it automatically.
pub(crate) struct CommitAdmission {
    cache_root: PathBuf,
    id: usize,
}

impl Drop for CommitAdmission {
    fn drop(&mut self) {
        let mut registry = pending_commit_registry().lock();
        if let Some(commits) = registry.by_root.get_mut(&self.cache_root) {
            commits.remove(&self.id);
            if commits.is_empty() {
                registry.by_root.remove(&self.cache_root);
            }
        }
    }
}

pub(crate) struct CacheQuota {
    limits: CacheLimits,
    gate: Mutex<()>,
    free_space_probe: Arc<dyn Fn(&Path) -> Result<u64> + Send + Sync>,
    #[cfg(test)]
    reservation_writes: AtomicUsize,
}

impl CacheQuota {
    pub(crate) fn new(limits: CacheLimits) -> Self {
        Self::with_free_space_probe(limits, Arc::new(os_available_space_bytes))
    }

    fn with_free_space_probe(
        limits: CacheLimits,
        free_space_probe: Arc<dyn Fn(&Path) -> Result<u64> + Send + Sync>,
    ) -> Self {
        Self {
            limits,
            gate: Mutex::new(()),
            free_space_probe,
            #[cfg(test)]
            reservation_writes: AtomicUsize::new(0),
        }
    }

    #[cfg(test)]
    fn new_with_free_space_probe(
        limits: CacheLimits,
        free_space_probe: Arc<dyn Fn(&Path) -> Result<u64> + Send + Sync>,
    ) -> Self {
        Self::with_free_space_probe(limits, free_space_probe)
    }

    pub(crate) fn limits(&self) -> &CacheLimits {
        &self.limits
    }

    pub(crate) fn maintain(&self, cache_dir: &Path, index: &dyn CacheIndex) -> Result<()> {
        self.maintain_at(cache_dir, index, unix_now_secs())
    }

    fn maintain_at(&self, cache_dir: &Path, index: &dyn CacheIndex, now_secs: u64) -> Result<()> {
        let _guard = self.gate.lock();
        let quota_lock = cache_quota_lock(cache_dir)?;
        let _quota_guard = quota_lock.lock();
        let root_lock = cache_root_lock(cache_dir)?;
        let _root_guard = root_lock.lock();
        let partial_dir = cache_dir.join(".partial");
        self.scan_reservations(&partial_dir, now_secs)?;
        self.reconcile_physical_blobs(cache_dir, index)
    }

    fn reconcile_physical_blobs(&self, cache_dir: &Path, index: &dyn CacheIndex) -> Result<()> {
        if !index.has_complete_lru_view() {
            return Ok(());
        }
        let entries = load_entries_lru(index)?;
        let mut referenced_paths = HashSet::new();
        for entry in entries {
            let Some(path) = entry
                .content_hash
                .parse::<Integrity>()
                .ok()
                .and_then(|sri| physical_blob_path(cache_dir, &sri))
            else {
                index.delete(&entry.key)?;
                continue;
            };
            if path.is_file() {
                referenced_paths.insert(path);
            } else {
                index.delete(&entry.key)?;
            }
        }

        let mut physical_files = Vec::new();
        collect_regular_files(&cache_dir.join("content-v2"), &mut physical_files)?;
        for path in physical_files {
            if !referenced_paths.contains(&path) {
                fs::remove_file(&path)
                    .with_context(|| format!("removing unindexed cache blob {path:?}"))?;
            }
        }
        Ok(())
    }

    pub(crate) fn reserve_at(
        &self,
        cache_dir: &Path,
        index: &dyn CacheIndex,
        owner: &CacheOwner,
        scoped_key: &str,
        reservation_path: &Path,
        target_bytes: u64,
        now_secs: u64,
    ) -> Result<()> {
        let _guard = self.gate.lock();
        let quota_lock = cache_quota_lock(cache_dir)?;
        let _quota_guard = quota_lock.lock();
        if target_bytes > self.limits.max_object_bytes {
            bail!(
                "cache per-object quota exceeded ({} > {} bytes)",
                target_bytes,
                self.limits.max_object_bytes
            );
        }

        let partial_dir = reservation_path
            .parent()
            .context("reservation path has no parent")?;
        let reservations = self.scan_reservations(partial_dir, now_secs)?;
        let reservation_is_new = !reservations
            .iter()
            .any(|(path, _)| path.as_path() == reservation_path);
        let other_owner_count = reservations
            .iter()
            .filter(|(path, record)| path.as_path() != reservation_path && &record.owner == owner)
            .count();
        if other_owner_count.saturating_add(usize::from(reservation_is_new))
            > self.limits.max_owner_partial_objects
        {
            bail!("cache owner partial object quota exceeded");
        }
        let other_global_count = reservations
            .iter()
            .filter(|(path, _)| path.as_path() != reservation_path)
            .count();
        if other_global_count.saturating_add(usize::from(reservation_is_new))
            > self.limits.max_global_partial_objects
        {
            bail!("cache global partial object quota exceeded");
        }

        let other_owner_partial = reservations
            .iter()
            .filter(|(path, record)| path.as_path() != reservation_path && &record.owner == owner)
            .map(|(_, record)| record.reserved_bytes)
            .fold(0_u64, u64::saturating_add);
        if other_owner_partial.saturating_add(target_bytes) > self.limits.max_owner_partial_bytes {
            bail!(
                "cache owner partial quota exceeded ({} > {} bytes)",
                other_owner_partial.saturating_add(target_bytes),
                self.limits.max_owner_partial_bytes
            );
        }

        let other_partial = reservations
            .iter()
            .filter(|(path, _)| path.as_path() != reservation_path)
            .map(|(_, record)| record.reserved_bytes)
            .fold(0_u64, u64::saturating_add);
        self.ensure_global_capacity(cache_dir, index, other_partial.saturating_add(target_bytes))?;

        let result = write_reservation(
            reservation_path,
            &ReservationRecord {
                owner: owner.clone(),
                scoped_key: scoped_key.to_string(),
                reserved_bytes: target_bytes,
                updated_at_secs: now_secs,
            },
        );
        #[cfg(test)]
        if result.is_ok() {
            self.reservation_writes.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    pub(crate) fn reserve(
        &self,
        cache_dir: &Path,
        index: &dyn CacheIndex,
        owner: &CacheOwner,
        scoped_key: &str,
        reservation_path: &Path,
        target_bytes: u64,
    ) -> Result<()> {
        self.reserve_at(
            cache_dir,
            index,
            owner,
            scoped_key,
            reservation_path,
            target_bytes,
            unix_now_secs(),
        )
    }

    #[cfg(test)]
    pub(crate) fn reservation_write_count(&self) -> usize {
        self.reservation_writes.load(Ordering::Relaxed)
    }

    pub(crate) fn release(&self, reservation_path: &Path) -> Result<()> {
        let _guard = self.gate.lock();
        let cache_dir = reservation_path
            .parent()
            .and_then(Path::parent)
            .context("reservation path is not inside a cache partial directory")?;
        let quota_lock = cache_quota_lock(cache_dir)?;
        let _quota_guard = quota_lock.lock();
        match fs::remove_file(reservation_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("removing cache reservation"),
        }
    }

    pub(crate) fn prepare_commit(
        &self,
        cache_dir: &Path,
        index: &dyn CacheIndex,
        owner: &CacheOwner,
        scoped_key: &str,
        incoming_bytes: u64,
    ) -> Result<CommitAdmission> {
        let _guard = self.gate.lock();
        let quota_lock = cache_quota_lock(cache_dir)?;
        let _quota_guard = quota_lock.lock();
        let root_lock = cache_root_lock(cache_dir)?;
        let _root_guard = root_lock.lock();
        let canonical_cache_root = fs::canonicalize(cache_dir)
            .with_context(|| format!("canonicalizing cache directory {cache_dir:?}"))?;
        let pending_commits: Vec<_> = pending_commit_registry()
            .lock()
            .by_root
            .get(&canonical_cache_root)
            .map(|commits| commits.values().cloned().collect())
            .unwrap_or_default();
        if incoming_bytes > self.limits.max_object_bytes {
            bail!("cache per-object quota exceeded");
        }
        if incoming_bytes > self.limits.max_owner_committed_bytes {
            bail!("cache owner committed quota exceeded");
        }

        let entries = load_entries_lru(index)?;
        let mut owner_groups_by_hash: HashMap<String, PhysicalBlob> = HashMap::new();
        for (position, entry) in entries.iter().enumerate() {
            if entry.key == scoped_key || entry_owner(&entry.key) != *owner {
                continue;
            }
            let group = owner_groups_by_hash
                .entry(entry.content_hash.clone())
                .or_insert_with(|| PhysicalBlob {
                    content_hash: entry.content_hash.clone(),
                    size_bytes: physical_blob_size(cache_dir, entry),
                    newest_lru_position: position,
                    keys: Vec::new(),
                });
            group.newest_lru_position = position;
            group.size_bytes = group.size_bytes.max(physical_blob_size(cache_dir, entry));
            group.keys.push(entry.key.clone());
        }
        let mut owner_groups: Vec<_> = owner_groups_by_hash.into_values().collect();
        owner_groups.sort_by_key(|group| group.newest_lru_position);
        let mut owner_bytes = owner_groups
            .iter()
            .fold(0_u64, |total, group| total.saturating_add(group.size_bytes));
        let pending_owner_bytes = pending_commits
            .iter()
            .filter(|commit| commit.owner == *owner && commit.scoped_key != scoped_key)
            .fold(0_u64, |total, commit| total.saturating_add(commit.bytes));
        let pending_owner_objects = pending_commits
            .iter()
            .filter(|commit| commit.owner == *owner && commit.scoped_key != scoped_key)
            .fold(0_usize, |total, commit| {
                total.saturating_add(commit.objects)
            });
        let incoming_objects = usize::from(!entries.iter().any(|entry| entry.key == scoped_key));
        let mut owner_objects = owner_groups.iter().fold(0_usize, |total, group| {
            total.saturating_add(group.keys.len())
        });
        let owner_within_limits = |bytes: u64, objects: usize| {
            bytes
                .saturating_add(pending_owner_bytes)
                .saturating_add(incoming_bytes)
                <= self.limits.max_owner_committed_bytes
                && objects
                    .saturating_add(pending_owner_objects)
                    .saturating_add(incoming_objects)
                    <= self.limits.max_owner_committed_objects
        };
        if !owner_within_limits(owner_bytes, owner_objects) {
            for group in owner_groups {
                for key in &group.keys {
                    index.delete(key)?;
                }
                owner_bytes = owner_bytes.saturating_sub(group.size_bytes);
                owner_objects = owner_objects.saturating_sub(group.keys.len());
                let hash_still_referenced = index.has_content_hash(&group.content_hash)?;
                if !hash_still_referenced {
                    remove_physical_blob(cache_dir, &group.content_hash)?;
                }
                if owner_within_limits(owner_bytes, owner_objects) {
                    break;
                }
            }
        }

        if !owner_within_limits(owner_bytes, owner_objects) {
            bail!("cache owner committed byte or object quota exceeded");
        }

        self.ensure_global_committed_capacity(
            cache_dir,
            index,
            scoped_key,
            incoming_bytes,
            &pending_commits,
        )?;

        let mut registry = pending_commit_registry().lock();
        let id = registry.next_id;
        registry.next_id = registry.next_id.wrapping_add(1);
        registry
            .by_root
            .entry(canonical_cache_root.clone())
            .or_default()
            .insert(
                id,
                PendingCommit {
                    owner: owner.clone(),
                    scoped_key: scoped_key.to_string(),
                    bytes: incoming_bytes,
                    objects: incoming_objects,
                },
            );
        Ok(CommitAdmission {
            cache_root: canonical_cache_root,
            id,
        })
    }

    fn ensure_global_committed_capacity(
        &self,
        cache_dir: &Path,
        index: &dyn CacheIndex,
        scoped_key: &str,
        incoming_bytes: u64,
        pending_commits: &[PendingCommit],
    ) -> Result<()> {
        let entries = load_entries_lru(index)?;
        let incoming_objects = usize::from(!entries.iter().any(|entry| entry.key == scoped_key));
        let pending_bytes = pending_commits
            .iter()
            .filter(|commit| commit.scoped_key != scoped_key)
            .fold(0_u64, |total, commit| total.saturating_add(commit.bytes));
        let pending_objects = pending_commits
            .iter()
            .filter(|commit| commit.scoped_key != scoped_key)
            .fold(0_usize, |total, commit| {
                total.saturating_add(commit.objects)
            });
        let mut committed_objects = entries
            .iter()
            .filter(|entry| entry.key != scoped_key)
            .count();

        let mut groups_by_hash: HashMap<String, PhysicalBlob> = HashMap::new();
        for (position, entry) in entries.iter().enumerate() {
            if entry.key == scoped_key {
                continue;
            }
            let group = groups_by_hash
                .entry(entry.content_hash.clone())
                .or_insert_with(|| PhysicalBlob {
                    content_hash: entry.content_hash.clone(),
                    size_bytes: physical_blob_size(cache_dir, entry),
                    newest_lru_position: position,
                    keys: Vec::new(),
                });
            group.newest_lru_position = position;
            group.size_bytes = group.size_bytes.max(physical_blob_size(cache_dir, entry));
            group.keys.push(entry.key.clone());
        }
        let mut groups: Vec<_> = groups_by_hash.into_values().collect();
        groups.sort_by_key(|group| group.newest_lru_position);
        let mut committed_bytes = groups
            .iter()
            .fold(0_u64, |total, group| total.saturating_add(group.size_bytes));
        let within_limits = |bytes: u64, objects: usize| {
            bytes
                .saturating_add(pending_bytes)
                .saturating_add(incoming_bytes)
                <= self.limits.max_global_bytes
                && objects
                    .saturating_add(pending_objects)
                    .saturating_add(incoming_objects)
                    <= self.limits.max_global_committed_objects
        };
        if within_limits(committed_bytes, committed_objects) {
            return Ok(());
        }

        for group in groups {
            for key in &group.keys {
                index.delete(key)?;
            }
            committed_bytes = committed_bytes.saturating_sub(group.size_bytes);
            committed_objects = committed_objects.saturating_sub(group.keys.len());
            let hash_still_referenced = index.has_content_hash(&group.content_hash)?;
            if !hash_still_referenced {
                remove_physical_blob(cache_dir, &group.content_hash)?;
            }
            if within_limits(committed_bytes, committed_objects) {
                return Ok(());
            }
        }

        bail!("cache global committed byte or object quota exceeded")
    }

    pub(crate) fn discard_blob_if_unreferenced(
        &self,
        cache_dir: &Path,
        index: &dyn CacheIndex,
        content_hash: &str,
    ) -> Result<()> {
        let _guard = self.gate.lock();
        let quota_lock = cache_quota_lock(cache_dir)?;
        let _quota_guard = quota_lock.lock();
        let root_lock = cache_root_lock(cache_dir)?;
        let _root_guard = root_lock.lock();
        Self::discard_blob_if_unreferenced_under_root(cache_dir, index, content_hash)
    }

    /// Remove a physical blob while the caller already holds the cache-root
    /// mutation lock. This supports atomic pattern deletion without trying to
    /// acquire the non-reentrant root lock a second time.
    pub(crate) fn discard_blob_if_unreferenced_under_root(
        cache_dir: &Path,
        index: &dyn CacheIndex,
        content_hash: &str,
    ) -> Result<()> {
        if index.has_content_hash(content_hash)? {
            return Ok(());
        }
        remove_physical_blob(cache_dir, content_hash)
    }

    fn scan_reservations(
        &self,
        partial_dir: &Path,
        now_secs: u64,
    ) -> Result<Vec<(PathBuf, ReservationRecord)>> {
        let mut records = Vec::new();
        let entries = match fs::read_dir(partial_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(records),
            Err(error) => return Err(error).context("reading cache partial directory"),
        };
        for entry in entries {
            let path = entry?.path();
            let is_atomic_temp = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.') && name.contains(".tmp-"));
            if is_atomic_temp {
                let modified_secs = file_modified_secs(&path);
                if now_secs.saturating_sub(modified_secs) > self.limits.partial_ttl.as_secs() {
                    fs::remove_file(&path)
                        .with_context(|| format!("removing stale cache temp file {path:?}"))?;
                }
                continue;
            }
            if path.extension().and_then(|extension| extension.to_str()) != Some("reservation") {
                continue;
            }
            let record = fs::metadata(&path)
                .ok()
                .filter(|metadata| metadata.len() <= MAX_RESERVATION_RECORD_BYTES)
                .and_then(|_| fs::read(&path).ok())
                .and_then(|json| serde_json::from_slice::<ReservationRecord>(&json).ok())
                .filter(|record| self.valid_reservation_record(record, now_secs));
            let Some(record) = record else {
                discard_sidecar_group(&path)?;
                continue;
            };
            if now_secs.saturating_sub(record.updated_at_secs) > self.limits.partial_ttl.as_secs() {
                discard_sidecar_group(&path)?;
                continue;
            }
            records.push((path, record));
        }

        let entries = fs::read_dir(partial_dir).context("re-reading cache partial directory")?;
        for entry in entries {
            let path = entry?.path();
            let extension = path.extension().and_then(|extension| extension.to_str());
            if !matches!(extension, Some("partial" | "meta")) {
                continue;
            }
            let reservation_path = path.with_extension("reservation");
            if reservation_path.exists() {
                continue;
            }
            let modified_secs = file_modified_secs(&path);
            if now_secs.saturating_sub(modified_secs) > self.limits.partial_ttl.as_secs() {
                discard_sidecar_group(&reservation_path)?;
            }
        }
        Ok(records)
    }

    fn valid_reservation_record(&self, record: &ReservationRecord, now_secs: u64) -> bool {
        if record.scoped_key.len() > MAX_RESERVATION_KEY_BYTES
            || record.reserved_bytes > self.limits.max_object_bytes
            || record.updated_at_secs > now_secs.saturating_add(self.limits.partial_ttl.as_secs())
        {
            return false;
        }
        if matches!(&record.owner, CacheOwner::Plugin(id) if id.len() > MAX_OWNER_ID_BYTES) {
            return false;
        }
        parse_scoped_key(&record.scoped_key).is_some_and(|(owner, _)| owner == record.owner)
    }

    fn ensure_global_capacity(
        &self,
        cache_dir: &Path,
        index: &dyn CacheIndex,
        reserved_bytes: u64,
    ) -> Result<()> {
        if reserved_bytes > self.limits.max_global_bytes {
            bail!("cache global quota exceeded");
        }

        let mut groups = physical_groups_lru(cache_dir, load_entries_lru(index)?);
        let mut committed_bytes = groups
            .iter()
            .fold(0_u64, |total, group| total.saturating_add(group.size_bytes));
        if self.global_capacity_fits(cache_dir, committed_bytes, reserved_bytes)? {
            return Ok(());
        }

        // Ordinary reservations never wait behind physical commits. Only a
        // pressure path takes the physical lock, reclaims crash orphans, and
        // recomputes the index view before evicting committed content.
        let root_lock = cache_root_lock(cache_dir)?;
        let _root_guard = root_lock.lock();
        self.reconcile_physical_blobs(cache_dir, index)?;
        groups = physical_groups_lru(cache_dir, load_entries_lru(index)?);
        committed_bytes = groups
            .iter()
            .fold(0_u64, |total, group| total.saturating_add(group.size_bytes));
        if self.global_capacity_fits(cache_dir, committed_bytes, reserved_bytes)? {
            return Ok(());
        }

        for group in groups {
            for key in &group.keys {
                index.delete(key)?;
            }
            remove_physical_blob(cache_dir, &group.content_hash)?;
            committed_bytes = committed_bytes.saturating_sub(group.size_bytes);
            if self.global_capacity_fits(cache_dir, committed_bytes, reserved_bytes)? {
                return Ok(());
            }
        }

        if committed_bytes.saturating_add(reserved_bytes) > self.limits.max_global_bytes {
            bail!("cache global quota exceeded");
        }
        bail!("cache free-space headroom exceeded")
    }

    fn global_capacity_fits(
        &self,
        cache_dir: &Path,
        committed_bytes: u64,
        reserved_bytes: u64,
    ) -> Result<bool> {
        if committed_bytes.saturating_add(reserved_bytes) > self.limits.max_global_bytes {
            return Ok(false);
        }
        let available_bytes = (self.free_space_probe)(cache_dir)
            .context("querying available cache filesystem space")?;
        Ok(reserved_bytes <= available_bytes.saturating_sub(self.limits.min_free_space_bytes))
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "redox"
))]
fn os_available_space_bytes(path: &Path) -> Result<u64> {
    let statistics = rustix::fs::statvfs(path).context("statvfs for cache directory")?;
    Ok(statistics.f_bavail.saturating_mul(statistics.f_frsize))
}

#[cfg(windows)]
fn os_available_space_bytes(path: &Path) -> Result<u64> {
    use std::iter;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide_path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect();
    let mut available_bytes = 0_u64;
    let success = unsafe {
        GetDiskFreeSpaceExW(
            wide_path.as_ptr(),
            &mut available_bytes,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if success == 0 {
        return Err(std::io::Error::last_os_error()).context("GetDiskFreeSpaceExW");
    }
    Ok(available_bytes)
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "redox",
    windows
)))]
fn os_available_space_bytes(_path: &Path) -> Result<u64> {
    Ok(u64::MAX)
}

fn entry_owner(key: &str) -> CacheOwner {
    parse_scoped_key(key)
        .map(|(owner, _)| owner)
        .unwrap_or(CacheOwner::Host)
}

fn load_entries_lru(index: &dyn CacheIndex) -> Result<Vec<arclain_db::CacheEntry>> {
    if !index.supports_lru_paging() {
        return index.entries_lru();
    }

    let mut entries = Vec::new();
    loop {
        let page = index.entries_lru_page(entries.len(), INDEX_PAGE_SIZE)?;
        let page_len = page.len();
        entries.extend(page);
        if page_len < INDEX_PAGE_SIZE {
            return Ok(entries);
        }
    }
}

fn physical_groups_lru(
    cache_dir: &Path,
    entries: Vec<arclain_db::CacheEntry>,
) -> Vec<PhysicalBlob> {
    let mut groups_by_hash: HashMap<String, PhysicalBlob> = HashMap::new();
    for (position, entry) in entries.into_iter().enumerate() {
        let group = groups_by_hash
            .entry(entry.content_hash.clone())
            .or_insert_with(|| PhysicalBlob {
                content_hash: entry.content_hash.clone(),
                size_bytes: physical_blob_size(cache_dir, &entry),
                newest_lru_position: position,
                keys: Vec::new(),
            });
        group.newest_lru_position = position;
        group.size_bytes = group.size_bytes.max(physical_blob_size(cache_dir, &entry));
        group.keys.push(entry.key);
    }
    let mut groups: Vec<_> = groups_by_hash.into_values().collect();
    groups.sort_by_key(|group| group.newest_lru_position);
    groups
}

struct PhysicalBlob {
    content_hash: String,
    size_bytes: u64,
    newest_lru_position: usize,
    keys: Vec<String>,
}

fn physical_blob_size(cache_dir: &Path, entry: &arclain_db::CacheEntry) -> u64 {
    entry
        .content_hash
        .parse::<Integrity>()
        .ok()
        .and_then(|sri| physical_blob_path(cache_dir, &sri))
        .and_then(|path| fs::metadata(path).ok())
        .map(|metadata| metadata.len())
        .or_else(|| entry.size_bytes.and_then(|size| u64::try_from(size).ok()))
        .unwrap_or(0)
}

fn physical_blob_path(cache_dir: &Path, sri: &Integrity) -> Option<PathBuf> {
    let (algorithm, hex) = sri.to_hex();
    (hex.len() >= 4).then(|| {
        cache_dir
            .join("content-v2")
            .join(algorithm.to_string())
            .join(&hex[..2])
            .join(&hex[2..4])
            .join(&hex[4..])
    })
}

fn collect_regular_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("reading cache tree {root:?}")),
    };
    for entry in entries {
        let path = entry?.path();
        let file_type = fs::symlink_metadata(&path)?.file_type();
        if file_type.is_dir() {
            collect_regular_files(&path, files)?;
        } else if file_type.is_file() || file_type.is_symlink() {
            files.push(path);
        }
    }
    Ok(())
}

fn remove_physical_blob(cache_dir: &Path, content_hash: &str) -> Result<()> {
    let Ok(sri) = content_hash.parse::<Integrity>() else {
        return Ok(());
    };
    match cacache::remove_hash_sync(cache_dir, &sri) {
        Ok(()) => Ok(()),
        Err(cacache::Error::IoError(error, _)) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn file_modified_secs(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_secs())
}

fn write_reservation(path: &Path, record: &ReservationRecord) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("creating cache partial directory")?;
    }
    let json = serde_json::to_vec(record).context("serializing cache reservation")?;
    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);
    let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("reservation path has no UTF-8 filename")?;
    let temp = path.with_file_name(format!(
        ".{file_name}.tmp-{}-{sequence}",
        std::process::id()
    ));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(&json)?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn discard_sidecar_group(reservation_path: &Path) -> Result<()> {
    for extension in ["reservation", "partial", "meta"] {
        let path = reservation_path.with_extension(extension);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("removing {path:?}")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::CacheIndex;
    use anyhow::Result;
    use arclain_db::{CacheEntry, CacheType};
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[derive(Default)]
    struct TestIndex {
        entries: Mutex<HashMap<String, CacheEntry>>,
    }

    impl CacheIndex for TestIndex {
        fn upsert(
            &self,
            key: &str,
            product_id: Option<&str>,
            content_hash: &str,
            source_url: Option<&str>,
            cache_type: CacheType,
            size_bytes: Option<i64>,
        ) -> Result<i64> {
            let mut entries = self.entries.lock();
            let id = entries.len() as i64 + 1;
            entries.insert(
                key.to_string(),
                CacheEntry {
                    id,
                    key: key.to_string(),
                    product_id: product_id.map(str::to_string),
                    content_hash: content_hash.to_string(),
                    source_url: source_url.map(str::to_string),
                    cache_type,
                    created_at: format!("{id:020}"),
                    last_accessed: None,
                    size_bytes,
                },
            );
            Ok(id)
        }
        fn get(&self, key: &str) -> Result<Option<CacheEntry>> {
            Ok(self.entries.lock().get(key).cloned())
        }
        fn has(&self, key: &str) -> Result<bool> {
            Ok(self.entries.lock().contains_key(key))
        }
        fn delete(&self, key: &str) -> Result<bool> {
            Ok(self.entries.lock().remove(key).is_some())
        }
        fn delete_by_pattern(&self, _pattern: &str) -> Result<usize> {
            Ok(0)
        }
        fn update_last_accessed(&self, _key: &str) -> Result<()> {
            Ok(())
        }
        fn entries_lru(&self) -> Result<Vec<CacheEntry>> {
            let mut entries: Vec<_> = self.entries.lock().values().cloned().collect();
            entries.sort_by_key(|entry| entry.id);
            Ok(entries)
        }

        fn has_complete_lru_view(&self) -> bool {
            true
        }
    }

    fn tight_limits() -> CacheLimits {
        CacheLimits {
            max_object_bytes: 8,
            max_owner_partial_bytes: 10,
            max_owner_committed_bytes: 16,
            max_global_bytes: 20,
            min_free_space_bytes: 0,
            partial_ttl: Duration::from_secs(10),
            ..CacheLimits::default()
        }
    }

    #[test]
    fn reservations_reject_per_object_and_concurrent_owner_aggregate_overflow() {
        let dir = tempfile::tempdir().unwrap();
        let partial_dir = dir.path().join(".partial");
        std::fs::create_dir_all(&partial_dir).unwrap();
        let quota = CacheQuota::new(tight_limits());
        let index: Arc<dyn CacheIndex> = Arc::new(TestIndex::default());
        let owner = CacheOwner::plugin("plugin-a");

        let object_error = quota
            .reserve_at(
                dir.path(),
                index.as_ref(),
                &owner,
                &owner.scoped_key("too-large"),
                &partial_dir.join("too-large.reservation"),
                9,
                100,
            )
            .unwrap_err();
        assert!(object_error.to_string().contains("per-object"));

        quota
            .reserve_at(
                dir.path(),
                index.as_ref(),
                &owner,
                &owner.scoped_key("one"),
                &partial_dir.join("one.reservation"),
                6,
                100,
            )
            .unwrap();
        let aggregate_error = quota
            .reserve_at(
                dir.path(),
                index.as_ref(),
                &owner,
                &owner.scoped_key("two"),
                &partial_dir.join("two.reservation"),
                5,
                100,
            )
            .unwrap_err();
        assert!(aggregate_error.to_string().contains("owner partial"));
    }

    #[test]
    fn zero_byte_reservations_still_obey_owner_and_global_object_counts() {
        let dir = tempfile::tempdir().unwrap();
        let partial_dir = dir.path().join(".partial");
        std::fs::create_dir_all(&partial_dir).unwrap();
        let mut limits = tight_limits();
        limits.max_owner_partial_objects = 1;
        limits.max_global_partial_objects = 2;
        let quota = CacheQuota::new(limits);
        let index = Arc::new(TestIndex::default());
        let owner = CacheOwner::plugin("plugin-a");

        quota
            .reserve_at(
                dir.path(),
                index.as_ref(),
                &owner,
                &owner.scoped_key("zero-one"),
                &partial_dir.join("zero-one.reservation"),
                0,
                100,
            )
            .unwrap();
        let error = quota
            .reserve_at(
                dir.path(),
                index.as_ref(),
                &owner,
                &owner.scoped_key("zero-two"),
                &partial_dir.join("zero-two.reservation"),
                0,
                100,
            )
            .unwrap_err();

        assert!(error.to_string().contains("partial object quota"));
    }

    #[test]
    fn oversized_reservation_records_are_discarded_before_accounting() {
        let dir = tempfile::tempdir().unwrap();
        let partial_dir = dir.path().join(".partial");
        std::fs::create_dir_all(&partial_dir).unwrap();
        let invalid_reservation = partial_dir.join("oversized.reservation");
        std::fs::write(
            &invalid_reservation,
            serde_json::to_vec(&serde_json::json!({
                "owner": { "Plugin": "plugin-a" },
                "scoped_key": "x".repeat(32 * 1024),
                "reserved_bytes": 0,
                "updated_at_secs": 100
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(partial_dir.join("oversized.partial"), b"partial").unwrap();
        std::fs::write(partial_dir.join("oversized.meta"), b"metadata").unwrap();
        let mut limits = tight_limits();
        limits.max_owner_partial_objects = 1;
        let quota = CacheQuota::new(limits);
        let index = Arc::new(TestIndex::default());
        let owner = CacheOwner::plugin("plugin-a");

        quota
            .reserve_at(
                dir.path(),
                index.as_ref(),
                &owner,
                &owner.scoped_key("fresh"),
                &partial_dir.join("fresh.reservation"),
                0,
                100,
            )
            .unwrap();

        assert!(!invalid_reservation.exists());
        assert!(!partial_dir.join("oversized.partial").exists());
        assert!(!partial_dir.join("oversized.meta").exists());
    }

    #[test]
    fn zero_byte_committed_rows_still_obey_owner_object_count() {
        let dir = tempfile::tempdir().unwrap();
        let mut limits = tight_limits();
        limits.max_owner_committed_objects = 2;
        limits.max_global_committed_objects = 10;
        let quota = CacheQuota::new(limits);
        let index = Arc::new(TestIndex::default());
        let owner = CacheOwner::plugin("plugin-a");
        for key in ["older", "newer"] {
            index
                .upsert(
                    &owner.scoped_key(key),
                    None,
                    &format!("invalid-{key}"),
                    None,
                    CacheType::Other,
                    Some(0),
                )
                .unwrap();
        }

        quota
            .prepare_commit(
                dir.path(),
                index.as_ref(),
                &owner,
                &owner.scoped_key("incoming"),
                0,
            )
            .unwrap();

        assert!(!index.has(&owner.scoped_key("older")).unwrap());
        assert!(index.has(&owner.scoped_key("newer")).unwrap());
    }

    #[test]
    fn zero_byte_committed_rows_still_obey_global_object_count() {
        let dir = tempfile::tempdir().unwrap();
        let mut limits = tight_limits();
        limits.max_owner_committed_objects = 10;
        limits.max_global_committed_objects = 2;
        let quota = CacheQuota::new(limits);
        let index = Arc::new(TestIndex::default());
        let owner_a = CacheOwner::plugin("a");
        let owner_b = CacheOwner::plugin("b");
        let owner_c = CacheOwner::plugin("c");
        for (owner, key) in [(&owner_a, "older"), (&owner_b, "newer")] {
            index
                .upsert(
                    &owner.scoped_key(key),
                    None,
                    &format!("invalid-{key}"),
                    None,
                    CacheType::Other,
                    Some(0),
                )
                .unwrap();
        }

        quota
            .prepare_commit(
                dir.path(),
                index.as_ref(),
                &owner_c,
                &owner_c.scoped_key("incoming"),
                0,
            )
            .unwrap();

        assert!(!index.has(&owner_a.scoped_key("older")).unwrap());
        assert!(index.has(&owner_b.scoped_key("newer")).unwrap());
    }

    #[test]
    fn stale_reservation_gc_removes_its_partial_and_resume_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let partial_dir = dir.path().join(".partial");
        std::fs::create_dir_all(&partial_dir).unwrap();
        let quota = CacheQuota::new(tight_limits());
        let index: Arc<dyn CacheIndex> = Arc::new(TestIndex::default());
        let owner = CacheOwner::plugin("plugin-a");
        let stale_reservation = partial_dir.join("stale.reservation");
        quota
            .reserve_at(
                dir.path(),
                index.as_ref(),
                &owner,
                &owner.scoped_key("stale"),
                &stale_reservation,
                6,
                80,
            )
            .unwrap();
        std::fs::write(partial_dir.join("stale.partial"), b"partial").unwrap();
        std::fs::write(partial_dir.join("stale.meta"), b"metadata").unwrap();

        quota
            .reserve_at(
                dir.path(),
                index.as_ref(),
                &owner,
                &owner.scoped_key("fresh"),
                &partial_dir.join("fresh.reservation"),
                5,
                100,
            )
            .unwrap();

        assert!(!stale_reservation.exists());
        assert!(!partial_dir.join("stale.partial").exists());
        assert!(!partial_dir.join("stale.meta").exists());
    }

    #[test]
    fn stale_legacy_partial_without_reservation_is_garbage_collected() {
        let dir = tempfile::tempdir().unwrap();
        let partial_dir = dir.path().join(".partial");
        std::fs::create_dir_all(&partial_dir).unwrap();
        let quota = CacheQuota::new(tight_limits());
        let index: Arc<dyn CacheIndex> = Arc::new(TestIndex::default());
        let orphan_partial = partial_dir.join("orphan.partial");
        let orphan_meta = partial_dir.join("orphan.meta");
        std::fs::write(&orphan_partial, b"partial").unwrap();
        std::fs::write(&orphan_meta, b"metadata").unwrap();
        let future = unix_now_secs() + tight_limits().partial_ttl.as_secs() + 1;

        quota
            .reserve_at(
                dir.path(),
                index.as_ref(),
                &CacheOwner::plugin("plugin-a"),
                &CacheOwner::plugin("plugin-a").scoped_key("fresh"),
                &partial_dir.join("fresh.reservation"),
                1,
                future,
            )
            .unwrap();

        assert!(!orphan_partial.exists());
        assert!(!orphan_meta.exists());
    }

    #[test]
    fn initialization_maintenance_removes_stale_atomic_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let partial_dir = dir.path().join(".partial");
        std::fs::create_dir_all(&partial_dir).unwrap();
        let reservation_temp = partial_dir.join(".item.reservation.tmp-dead");
        let metadata_temp = partial_dir.join(".item.meta.tmp-dead");
        std::fs::write(&reservation_temp, b"temp").unwrap();
        std::fs::write(&metadata_temp, b"temp").unwrap();
        let quota = CacheQuota::new(tight_limits());
        let index = Arc::new(TestIndex::default());
        let future = unix_now_secs() + tight_limits().partial_ttl.as_secs() + 1;

        quota
            .maintain_at(dir.path(), index.as_ref(), future)
            .unwrap();

        assert!(!reservation_temp.exists());
        assert!(!metadata_temp.exists());
    }

    #[test]
    fn global_quota_evicts_lru_physical_blob_and_all_shared_hash_references() {
        let dir = tempfile::tempdir().unwrap();
        let partial_dir = dir.path().join(".partial");
        std::fs::create_dir_all(&partial_dir).unwrap();
        let limits = CacheLimits {
            max_object_bytes: 8,
            max_owner_partial_bytes: 8,
            max_owner_committed_bytes: 32,
            max_global_bytes: 8,
            min_free_space_bytes: 0,
            partial_ttl: Duration::from_secs(10),
            ..CacheLimits::default()
        };
        let quota = CacheQuota::new(limits);
        let index = Arc::new(TestIndex::default());
        let owner_a = CacheOwner::plugin("a");
        let owner_b = CacheOwner::plugin("b");
        let shared_sri = cacache::write_hash_sync(dir.path(), b"shared").unwrap();
        let other_sri = cacache::write_hash_sync(dir.path(), b"zz").unwrap();
        index
            .upsert(
                &owner_a.scoped_key("old-a"),
                None,
                &shared_sri.to_string(),
                None,
                CacheType::Other,
                Some(6),
            )
            .unwrap();
        index
            .upsert(
                &owner_b.scoped_key("old-b"),
                None,
                &shared_sri.to_string(),
                None,
                CacheType::Other,
                Some(6),
            )
            .unwrap();
        index
            .upsert(
                &owner_b.scoped_key("newer"),
                None,
                &other_sri.to_string(),
                None,
                CacheType::Other,
                Some(2),
            )
            .unwrap();

        quota
            .reserve_at(
                dir.path(),
                index.as_ref(),
                &owner_a,
                &owner_a.scoped_key("incoming"),
                &partial_dir.join("incoming.reservation"),
                4,
                100,
            )
            .unwrap();

        assert!(!index.has(&owner_a.scoped_key("old-a")).unwrap());
        assert!(!index.has(&owner_b.scoped_key("old-b")).unwrap());
        assert!(index.has(&owner_b.scoped_key("newer")).unwrap());
        assert!(cacache::SyncReader::open_hash(dir.path(), shared_sri).is_err());
        assert!(cacache::SyncReader::open_hash(dir.path(), other_sri).is_ok());
    }

    #[test]
    fn free_space_headroom_caps_new_reservations() {
        let dir = tempfile::tempdir().unwrap();
        let partial_dir = dir.path().join(".partial");
        std::fs::create_dir_all(&partial_dir).unwrap();
        let limits = CacheLimits {
            max_object_bytes: 10,
            max_owner_partial_bytes: 20,
            max_global_bytes: 100,
            min_free_space_bytes: 20,
            ..CacheLimits::default()
        };
        let quota = CacheQuota::new_with_free_space_probe(limits, Arc::new(|_| Ok(25)));
        let index = Arc::new(TestIndex::default());
        let owner = CacheOwner::plugin("a");

        quota
            .reserve(
                dir.path(),
                index.as_ref(),
                &owner,
                &owner.scoped_key("fits"),
                &partial_dir.join("fits.reservation"),
                5,
            )
            .unwrap();
        quota
            .release(&partial_dir.join("fits.reservation"))
            .unwrap();
        let error = quota
            .reserve(
                dir.path(),
                index.as_ref(),
                &owner,
                &owner.scoped_key("too-large"),
                &partial_dir.join("too-large.reservation"),
                6,
            )
            .unwrap_err();

        assert!(error.to_string().contains("free-space headroom"));
    }

    #[test]
    fn owner_committed_quota_evicts_only_that_owners_lru_entry() {
        let dir = tempfile::tempdir().unwrap();
        let limits = CacheLimits {
            max_object_bytes: 8,
            max_owner_partial_bytes: 16,
            max_owner_committed_bytes: 6,
            max_global_bytes: 64,
            partial_ttl: Duration::from_secs(10),
            ..CacheLimits::default()
        };
        let quota = CacheQuota::new(limits);
        let index = Arc::new(TestIndex::default());
        let owner_a = CacheOwner::plugin("a");
        let owner_b = CacheOwner::plugin("b");
        let old_a_sri = cacache::write_hash_sync(dir.path(), b"aaaaaa").unwrap();
        let b_sri = cacache::write_hash_sync(dir.path(), b"bbbbbb").unwrap();
        index
            .upsert(
                &owner_a.scoped_key("old"),
                None,
                &old_a_sri.to_string(),
                None,
                CacheType::Other,
                Some(6),
            )
            .unwrap();
        index
            .upsert(
                &owner_b.scoped_key("keep"),
                None,
                &b_sri.to_string(),
                None,
                CacheType::Other,
                Some(6),
            )
            .unwrap();

        quota
            .prepare_commit(
                dir.path(),
                index.as_ref(),
                &owner_a,
                &owner_a.scoped_key("incoming"),
                6,
            )
            .unwrap();

        assert!(!index.has(&owner_a.scoped_key("old")).unwrap());
        assert!(index.has(&owner_b.scoped_key("keep")).unwrap());
        assert!(cacache::SyncReader::open_hash(dir.path(), old_a_sri).is_err());
        assert!(cacache::SyncReader::open_hash(dir.path(), b_sri).is_ok());
    }

    #[test]
    fn owner_committed_quota_counts_shared_physical_hash_once() {
        let dir = tempfile::tempdir().unwrap();
        let limits = CacheLimits {
            max_object_bytes: 8,
            max_owner_partial_bytes: 16,
            max_owner_committed_bytes: 10,
            max_global_bytes: 64,
            partial_ttl: Duration::from_secs(10),
            ..CacheLimits::default()
        };
        let quota = CacheQuota::new(limits);
        let index = Arc::new(TestIndex::default());
        let owner = CacheOwner::plugin("a");
        let shared_sri = cacache::write_hash_sync(dir.path(), b"shared").unwrap();
        for key in ["older", "newest"] {
            index
                .upsert(
                    &owner.scoped_key(key),
                    None,
                    &shared_sri.to_string(),
                    None,
                    CacheType::Other,
                    Some(6),
                )
                .unwrap();
        }

        quota
            .prepare_commit(
                dir.path(),
                index.as_ref(),
                &owner,
                &owner.scoped_key("incoming"),
                4,
            )
            .unwrap();

        assert!(index.has(&owner.scoped_key("older")).unwrap());
        assert!(index.has(&owner.scoped_key("newest")).unwrap());
        assert!(cacache::SyncReader::open_hash(dir.path(), shared_sri).is_ok());
    }
}
