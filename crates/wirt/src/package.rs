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
const EOCD_BYTES: usize = 22;
const ZIP64_LOCATOR_BYTES: usize = 20;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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

impl std::str::FromStr for PackageFingerprint {
    type Err = PluginError;

    fn from_str(value: &str) -> Result<Self> {
        if value.len() != 64
            || !value
                .as_bytes()
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        {
            return Err(package_error(
                "package fingerprint must be exactly 64 lowercase hexadecimal characters",
            ));
        }
        Ok(Self(value.to_owned()))
    }
}

impl<'de> Deserialize<'de> for PackageFingerprint {
    fn deserialize<Deserializer>(
        deserializer: Deserializer,
    ) -> std::result::Result<Self, Deserializer::Error>
    where
        Deserializer: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
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
    PluginError::InvalidPackage(message.into())
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
    let contract =
        inspect_component_contract(component).map_err(|error| package_error(error.to_string()))?;
    if contract.abi != parsed.wirt.abi {
        return Err(package_error("manifest and component ABI differ"));
    }

    write_package_bytes(manifest, component)
}

fn write_package_bytes(manifest: &[u8], component: &[u8]) -> Result<Vec<u8>> {
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
    validate_archive_envelope(bytes)?;
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| package_error("archive-structure: invalid ZIP archive"))?;
    if archive.len() != 2 {
        return Err(package_error(
            "archive-structure: archive must contain exactly two entries",
        ));
    }
    let manifest_bytes = read_entry(&mut archive, 0, "plugin.toml", MAX_PLUGIN_MANIFEST_BYTES)?;
    let component = read_entry(&mut archive, 1, "plugin.wasm", MAX_PLUGIN_WASM_BYTES)?;
    let manifest = parse_manifest(&manifest_bytes)?;
    validate_manifest(&manifest)?;
    let canonical = write_package_bytes(&manifest_bytes, &component)?;
    if canonical != bytes {
        return Err(package_error(
            "archive-canonical: archive bytes do not match the canonical Wirt encoding",
        ));
    }
    let contract =
        inspect_component_contract(&component).map_err(|error| package_error(error.to_string()))?;
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

fn validate_archive_envelope(bytes: &[u8]) -> Result<()> {
    const EOCD_SIGNATURE: &[u8; 4] = b"PK\x05\x06";
    const CENTRAL_SIGNATURE: &[u8; 4] = b"PK\x01\x02";
    const ZIP64_LOCATOR_SIGNATURE: &[u8; 4] = b"PK\x06\x07";

    let search_start = bytes.len().saturating_sub(EOCD_BYTES + u16::MAX as usize);
    let eocd = bytes
        .get(search_start..)
        .unwrap_or_default()
        .windows(EOCD_SIGNATURE.len())
        .rposition(|window| window == EOCD_SIGNATURE)
        .map(|relative| search_start + relative)
        .ok_or_else(|| package_error("archive-envelope: missing-eocd"))?;
    let record = bytes
        .get(eocd..eocd.saturating_add(EOCD_BYTES))
        .ok_or_else(|| package_error("archive-envelope: truncated-eocd"))?;

    let disk = u16::from_le_bytes([record[4], record[5]]);
    let central_disk = u16::from_le_bytes([record[6], record[7]]);
    let entries_on_disk = u16::from_le_bytes([record[8], record[9]]);
    let entries = u16::from_le_bytes([record[10], record[11]]);
    let central_size = u32::from_le_bytes(record[12..16].try_into().unwrap());
    let central_offset = u32::from_le_bytes(record[16..20].try_into().unwrap());
    let comment_size = u16::from_le_bytes([record[20], record[21]]) as usize;

    if eocd >= ZIP64_LOCATOR_BYTES
        && bytes.get(eocd - ZIP64_LOCATOR_BYTES..eocd - ZIP64_LOCATOR_BYTES + 4)
            == Some(ZIP64_LOCATOR_SIGNATURE)
    {
        return Err(package_error("archive-envelope: zip64"));
    }
    if disk != 0 || central_disk != 0 {
        return Err(package_error("archive-envelope: multidisk"));
    }
    if entries == u16::MAX || central_size == u32::MAX || central_offset == u32::MAX {
        return Err(package_error("archive-envelope: zip64"));
    }
    if entries != 2 || entries_on_disk != entries {
        return Err(package_error("archive-envelope: entry-count"));
    }
    if comment_size != 0 || eocd.checked_add(EOCD_BYTES) != Some(bytes.len()) {
        return Err(package_error("archive-envelope: comments-or-trailing-data"));
    }

    let central_offset = central_offset as usize;
    let central_size = central_size as usize;
    let central_end = central_offset
        .checked_add(central_size)
        .ok_or_else(|| package_error("archive-envelope: central-directory-bounds"))?;
    if central_end != eocd
        || bytes.get(central_offset..central_offset.saturating_add(4)) != Some(CENTRAL_SIGNATURE)
    {
        return Err(package_error("archive-envelope: central-directory-bounds"));
    }
    Ok(())
}

fn parse_manifest(bytes: &[u8]) -> Result<PluginManifest> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| package_error("manifest-parse: manifest is not UTF-8"))?;
    toml::from_str(text).map_err(|_| package_error("manifest-parse: invalid TOML manifest"))
}

fn read_entry(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    index: usize,
    expected_name: &str,
    max: usize,
) -> Result<Vec<u8>> {
    let mut entry = archive
        .by_index(index)
        .map_err(|_| package_error("archive-structure: invalid central-directory entry"))?;
    if entry.name_raw() != expected_name.as_bytes()
        || entry.compression() != CompressionMethod::Deflated
        || entry.encrypted()
        || entry.is_dir()
    {
        return Err(package_error(format!(
            "archive-structure: unsafe metadata for {expected_name}"
        )));
    }
    if let Some(mode) = entry.unix_mode() {
        let kind = mode & 0o170000;
        if kind != 0 && kind != 0o100000 {
            return Err(package_error(format!(
                "archive-structure: {expected_name} is not a regular file"
            )));
        }
    }
    if entry.size() > max as u64 {
        return Err(package_error(format!(
            "archive-bounds: {expected_name} exceeds its expanded-byte limit"
        )));
    }
    if entry.size() >= MIN_RATIO_CHECK_BYTES
        && entry.size() > entry.compressed_size().saturating_mul(MAX_EXPANSION_RATIO)
    {
        return Err(package_error(format!(
            "archive-bounds: {expected_name} exceeds the expansion ratio limit"
        )));
    }
    let declared = entry.size();
    let mut result = Vec::with_capacity(declared.min(max as u64) as usize);
    entry
        .by_ref()
        .take(max as u64 + 1)
        .read_to_end(&mut result)
        .map_err(|_| {
            package_error(format!(
                "archive-structure: failed to expand {expected_name}"
            ))
        })?;
    if result.len() > max || result.len() as u64 != declared {
        return Err(package_error(format!(
            "archive-bounds: dishonest expanded size for {expected_name}"
        )));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_fingerprint_parses_only_exact_lowercase_sha256() {
        let expected = "0123456789abcdef".repeat(4);
        let parsed: PackageFingerprint = expected.parse().expect("valid SHA-256");
        assert_eq!(parsed.as_str(), expected);

        for invalid in [
            "0".repeat(63),
            "0".repeat(65),
            "ABCDEF0123456789".repeat(4),
            "g123456789abcdef".repeat(4),
        ] {
            assert!(
                invalid.parse::<PackageFingerprint>().is_err(),
                "accepted malformed fingerprint {invalid:?}",
            );
        }
    }

    #[test]
    fn package_fingerprint_deserialization_uses_the_strict_parser() {
        let expected = "0123456789abcdef".repeat(4);
        let parsed: PackageFingerprint =
            serde_json::from_str(&serde_json::to_string(&expected).unwrap()).unwrap();
        assert_eq!(parsed.as_str(), expected);

        for invalid in ["ABC".to_string(), "0".repeat(63), "A".repeat(64)] {
            assert!(
                serde_json::from_str::<PackageFingerprint>(
                    &serde_json::to_string(&invalid).unwrap()
                )
                .is_err(),
                "deserialized malformed fingerprint {invalid:?}",
            );
        }
    }

    #[test]
    fn deterministic_writer_has_a_small_inline_fixed_output_golden() {
        const MANIFEST: &[u8] = b"[wirt]\nabi = \"0.2.0\"\n";
        const COMPONENT: &[u8] = b"\0asm\x0d\0\x01\0";
        let package = write_package_bytes(MANIFEST, COMPONENT).unwrap();
        assert_eq!(
            PackageFingerprint::sha256(&package).as_str(),
            "8f8415c612b3e08fe18e039e9ef303fbcd187bf865b9fcfe7a63e41b7c3cbe44"
        );
    }
}
