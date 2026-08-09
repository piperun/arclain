use crate::component_contract::inspect_component_contract;
use crate::loader::validate_manifest;
use crate::{PluginError, PluginManifest, Result, WIRT_ABI_VERSION};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub const MAX_WIRT_PACKAGE_BYTES: u64 = 65 * 1024 * 1024;
pub const MAX_PLUGIN_MANIFEST_BYTES: usize = 64 * 1024;
pub const MAX_PLUGIN_WASM_BYTES: usize = 64 * 1024 * 1024;
const MAX_EXPANSION_RATIO: u64 = 1_000;
const MIN_RATIO_CHECK_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PackageFingerprint(String);

impl PackageFingerprint {
    pub fn sha256(bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        Self(digest.iter().map(|byte| format!("{byte:02x}")).collect())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PackageFingerprint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug)]
pub struct ValidatedPackage {
    pub manifest: PluginManifest,
    pub manifest_bytes: Vec<u8>,
    pub component: Vec<u8>,
    pub fingerprint: PackageFingerprint,
}

fn package_error(message: impl Into<String>) -> PluginError {
    PluginError::LoadError(format!("invalid Wirt package: {}", message.into()))
}

fn checked_input<'a>(bytes: &'a [u8], max: usize, kind: &str) -> Result<&'a [u8]> {
    if bytes.len() > max {
        return Err(package_error(format!(
            "{kind} exceeds the {max}-byte limit"
        )));
    }
    Ok(bytes)
}

pub fn package_bytes(manifest: &[u8], component: &[u8]) -> Result<Vec<u8>> {
    checked_input(manifest, MAX_PLUGIN_MANIFEST_BYTES, "plugin manifest")?;
    checked_input(component, MAX_PLUGIN_WASM_BYTES, "plugin component")?;
    let parsed = parse_manifest(manifest)?;
    validate_manifest(&parsed)?;
    let contract = inspect_component_contract(component)?;
    if contract.abi != parsed.wirt.abi {
        return Err(package_error("manifest and component ABI differ"));
    }

    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(9))
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644)
        .large_file(false);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file("plugin.toml", options)
        .map_err(|error| package_error(error.to_string()))?;
    writer.write_all(manifest)?;
    writer
        .start_file("plugin.wasm", options)
        .map_err(|error| package_error(error.to_string()))?;
    writer.write_all(component)?;
    let bytes = writer
        .finish()
        .map_err(|error| package_error(error.to_string()))?
        .into_inner();
    if bytes.len() as u64 > MAX_WIRT_PACKAGE_BYTES {
        return Err(package_error("archive exceeds the 65-MiB limit"));
    }
    Ok(bytes)
}

pub fn read_package(path: &Path) -> Result<ValidatedPackage> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(package_error("package path is not a regular file"));
    }
    if metadata.len() > MAX_WIRT_PACKAGE_BYTES {
        return Err(package_error("archive exceeds the 65-MiB limit"));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_WIRT_PACKAGE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    read_package_bytes(&bytes)
}

pub fn read_package_bytes(bytes: &[u8]) -> Result<ValidatedPackage> {
    if bytes.len() as u64 > MAX_WIRT_PACKAGE_BYTES {
        return Err(package_error("archive exceeds the 65-MiB limit"));
    }
    validate_canonical_zip_headers(bytes)?;
    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|error| package_error(error.to_string()))?;
    if archive.len() != 2 || !archive.comment().is_empty() {
        return Err(package_error(
            "archive must contain exactly two entries and no comment",
        ));
    }
    let manifest_bytes = read_entry(&mut archive, 0, "plugin.toml", MAX_PLUGIN_MANIFEST_BYTES)?;
    let component = read_entry(&mut archive, 1, "plugin.wasm", MAX_PLUGIN_WASM_BYTES)?;
    let manifest = parse_manifest(&manifest_bytes)?;
    validate_manifest(&manifest)?;
    let contract = inspect_component_contract(&component)?;
    if contract.abi != manifest.wirt.abi || manifest.wirt.abi != WIRT_ABI_VERSION {
        return Err(package_error("manifest, component, and host ABI differ"));
    }
    Ok(ValidatedPackage {
        manifest,
        manifest_bytes,
        component,
        fingerprint: PackageFingerprint::sha256(bytes),
    })
}

fn parse_manifest(bytes: &[u8]) -> Result<PluginManifest> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| package_error(format!("manifest is not UTF-8: {error}")))?;
    toml::from_str(text).map_err(Into::into)
}

fn read_entry(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    index: usize,
    expected_name: &str,
    max: usize,
) -> Result<Vec<u8>> {
    let mut entry = archive
        .by_index(index)
        .map_err(|error| package_error(error.to_string()))?;
    if entry.name_raw() != expected_name.as_bytes()
        || entry.compression() != CompressionMethod::Deflated
        || entry.unix_mode() != Some(0o100644)
        || !entry.comment().is_empty()
        || !entry.extra_data().unwrap_or_default().is_empty()
    {
        return Err(package_error(format!(
            "noncanonical metadata for {expected_name}"
        )));
    }
    if entry.size() > max as u64 {
        return Err(package_error(format!(
            "{expected_name} exceeds its expanded-byte limit"
        )));
    }
    if entry.size() >= MIN_RATIO_CHECK_BYTES
        && entry.size() > entry.compressed_size().saturating_mul(MAX_EXPANSION_RATIO)
    {
        return Err(package_error(format!(
            "{expected_name} exceeds the expansion ratio limit"
        )));
    }
    let declared = entry.size();
    let mut result = Vec::with_capacity(declared.min(max as u64) as usize);
    entry
        .by_ref()
        .take(max as u64 + 1)
        .read_to_end(&mut result)
        .map_err(|error| package_error(format!("failed to expand {expected_name}: {error}")))?;
    if result.len() > max || result.len() as u64 != declared {
        return Err(package_error(format!(
            "dishonest expanded size for {expected_name}"
        )));
    }
    Ok(result)
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16> {
    bytes
        .get(at..at + 2)
        .map(|value| u16::from_le_bytes([value[0], value[1]]))
        .ok_or_else(|| package_error("truncated ZIP header"))
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32> {
    bytes
        .get(at..at + 4)
        .map(|value| u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
        .ok_or_else(|| package_error("truncated ZIP header"))
}

fn validate_canonical_zip_headers(bytes: &[u8]) -> Result<()> {
    let names = [b"plugin.toml".as_slice(), b"plugin.wasm".as_slice()];
    let mut local_sizes = [(0_u32, 0_u32, 0_u32); 2];
    let mut offset = 0_usize;
    for (index, expected) in names.into_iter().enumerate() {
        if bytes.get(offset..offset + 4) != Some(b"PK\x03\x04") {
            return Err(package_error("missing canonical local file header"));
        }
        let flags = u16_at(bytes, offset + 6)?;
        let method = u16_at(bytes, offset + 8)?;
        let time = u16_at(bytes, offset + 10)?;
        let date = u16_at(bytes, offset + 12)?;
        if flags & !0x0006 != 0 || method != 8 || time != 0 || date != 0x21 {
            return Err(package_error(format!(
                "encryption, descriptors, or noncanonical ZIP metadata (flags={flags:#06x}, method={method}, time={time:#06x}, date={date:#06x})"
            )));
        }
        let compressed = u32_at(bytes, offset + 18)? as usize;
        local_sizes[index] = (
            u32_at(bytes, offset + 14)?,
            compressed as u32,
            u32_at(bytes, offset + 22)?,
        );
        let name_len = u16_at(bytes, offset + 26)? as usize;
        let extra_len = u16_at(bytes, offset + 28)? as usize;
        let name_at = offset + 30;
        if extra_len != 0 || bytes.get(name_at..name_at + name_len) != Some(expected) {
            return Err(package_error("noncanonical entry name or extra field"));
        }
        offset = name_at
            .checked_add(name_len)
            .and_then(|value| value.checked_add(compressed))
            .ok_or_else(|| package_error("ZIP offset overflow"))?;
    }

    for (index, expected) in names.into_iter().enumerate() {
        if bytes.get(offset..offset + 4) != Some(b"PK\x01\x02") {
            return Err(package_error("missing canonical central directory header"));
        }
        if u16_at(bytes, offset + 8)? & !0x0006 != 0
            || u16_at(bytes, offset + 10)? != 8
            || u16_at(bytes, offset + 12)? != 0
            || u16_at(bytes, offset + 14)? != 0x21
        {
            return Err(package_error(
                "noncanonical central directory flags or metadata",
            ));
        }
        let name_len = u16_at(bytes, offset + 28)? as usize;
        let extra_len = u16_at(bytes, offset + 30)? as usize;
        let comment_len = u16_at(bytes, offset + 32)? as usize;
        let mode = u32_at(bytes, offset + 38)? >> 16;
        let central_sizes = (
            u32_at(bytes, offset + 16)?,
            u32_at(bytes, offset + 20)?,
            u32_at(bytes, offset + 24)?,
        );
        let name_at = offset + 46;
        if extra_len != 0
            || comment_len != 0
            || mode != 0o100644
            || central_sizes != local_sizes[index]
            || bytes.get(name_at..name_at + name_len) != Some(expected)
        {
            return Err(package_error("noncanonical central directory entry"));
        }
        offset = name_at
            .checked_add(name_len)
            .and_then(|value| value.checked_add(extra_len))
            .and_then(|value| value.checked_add(comment_len))
            .ok_or_else(|| package_error("ZIP offset overflow"))?;
    }
    if bytes.get(offset..offset + 4) != Some(b"PK\x05\x06")
        || bytes.len() != offset + 22
        || u16_at(bytes, offset + 8)? != 2
        || u16_at(bytes, offset + 10)? != 2
        || u16_at(bytes, offset + 20)? != 0
    {
        return Err(package_error(
            "ZIP64, trailing data, or archive comments are not allowed",
        ));
    }
    Ok(())
}
