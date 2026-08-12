mod support;

use std::io::{Cursor, Write};
use support::{manifest_toml, UI_DEMO_COMPONENT};
use wirt::{
    package_bytes, read_package_bytes, MAX_PLUGIN_MANIFEST_BYTES, MAX_PLUGIN_WASM_BYTES,
    MAX_WIRT_PACKAGE_BYTES, WIRT_ABI_VERSION,
};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

fn archive(entries: &[(&str, &[u8], SimpleFileOptions)]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes, options) in entries {
        writer.start_file(*name, *options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn canonical_options() -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(9))
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644)
}

fn package_with_names(first: &str, second: &str) -> Vec<u8> {
    archive(&[
        (first, manifest_toml().as_bytes(), canonical_options()),
        (second, UI_DEMO_COMPONENT, canonical_options()),
    ])
}

fn duplicate_name_archive() -> Vec<u8> {
    let mut bytes = package_with_names("plugin.toml", "plugin.wasm");
    for offset in 0..=bytes.len() - b"plugin.wasm".len() {
        if &bytes[offset..offset + b"plugin.wasm".len()] == b"plugin.wasm" {
            bytes[offset..offset + b"plugin.toml".len()].copy_from_slice(b"plugin.toml");
        }
    }
    bytes
}

fn signature_offsets(bytes: &[u8], signature: [u8; 4]) -> Vec<usize> {
    bytes
        .windows(4)
        .enumerate()
        .filter_map(|(offset, window)| (window == signature).then_some(offset))
        .collect()
}

fn assert_archive_preflight_rejection(label: &str, bytes: &[u8]) {
    let error = read_package_bytes(bytes).unwrap_err().to_string();
    assert!(
        error.contains("archive-envelope:")
            || error.contains("archive-canonical:")
            || error.contains("archive-structure:"),
        "{label} escaped archive preflight: {error}"
    );
}

fn assert_envelope_rejection(label: &str, bytes: &[u8]) {
    let error = read_package_bytes(bytes).unwrap_err().to_string();
    assert!(
        error.contains("archive-envelope:"),
        "{label} escaped raw envelope validation: {error}"
    );
}

#[test]
fn rejects_noncanonical_entry_sets_and_paths() {
    for (label, bytes) in [
        (
            "extra entry",
            archive(&[
                (
                    "plugin.toml",
                    manifest_toml().as_bytes(),
                    canonical_options(),
                ),
                ("plugin.wasm", UI_DEMO_COMPONENT, canonical_options()),
                ("extra", b"x", canonical_options()),
            ]),
        ),
        ("duplicate entry", duplicate_name_archive()),
        (
            "case collision",
            package_with_names("PLUGIN.TOML", "plugin.toml"),
        ),
        (
            "directory",
            package_with_names("plugin.toml/", "plugin.wasm"),
        ),
        (
            "slash",
            package_with_names("nested/plugin.toml", "plugin.wasm"),
        ),
        (
            "backslash",
            package_with_names("nested\\plugin.toml", "plugin.wasm"),
        ),
        (
            "absolute",
            package_with_names("/plugin.toml", "plugin.wasm"),
        ),
        (
            "drive absolute",
            package_with_names("C:\\plugin.toml", "plugin.wasm"),
        ),
        (
            "traversal",
            package_with_names("../plugin.toml", "plugin.wasm"),
        ),
    ] {
        assert_archive_preflight_rejection(label, &bytes);
    }
}

#[test]
fn rejects_noncanonical_zip_metadata_before_component_validation() {
    let stored = archive(&[
        (
            "plugin.toml",
            manifest_toml().as_bytes(),
            canonical_options()
                .compression_method(CompressionMethod::Stored)
                .compression_level(None),
        ),
        ("plugin.wasm", b"not a component", canonical_options()),
    ]);
    assert_archive_preflight_rejection("stored entry", &stored);

    let linked = archive(&[
        (
            "plugin.toml",
            manifest_toml().as_bytes(),
            canonical_options().unix_permissions(0o777),
        ),
        ("plugin.wasm", b"not a component", canonical_options()),
    ]);
    assert_archive_preflight_rejection("noncanonical permissions", &linked);

    let zip64 = archive(&[
        (
            "plugin.toml",
            manifest_toml().as_bytes(),
            canonical_options().large_file(true),
        ),
        ("plugin.wasm", b"not a component", canonical_options()),
    ]);
    assert_archive_preflight_rejection("forced ZIP64 entry", &zip64);

    for (label, mode) in [("link", 0o120777_u32), ("device", 0o020666_u32)] {
        let mut bytes = package_with_names("plugin.toml", "plugin.wasm");
        let central = signature_offsets(&bytes, [0x50, 0x4b, 0x01, 0x02])[0];
        bytes[central + 38..central + 42].copy_from_slice(&(mode << 16).to_le_bytes());
        assert_archive_preflight_rejection(label, &bytes);
    }
}

#[test]
fn rejects_encryption_and_unknown_size_descriptor_flags() {
    let base = package_with_names("plugin.toml", "plugin.wasm");
    for (label, flag) in [("encryption", 0x0001_u16), ("descriptor", 0x0008_u16)] {
        let mut bytes = base.clone();
        for signature in [[0x50, 0x4b, 0x03, 0x04], [0x50, 0x4b, 0x01, 0x02]] {
            let mut offset = 0;
            while let Some(found) = bytes[offset..]
                .windows(4)
                .position(|window| window == signature)
            {
                let header = offset + found;
                let flags_at = header + if signature[2] == 0x03 { 6 } else { 8 };
                let flags = u16::from_le_bytes([bytes[flags_at], bytes[flags_at + 1]]) | flag;
                bytes[flags_at..flags_at + 2].copy_from_slice(&flags.to_le_bytes());
                offset = header + 4;
            }
        }
        assert_archive_preflight_rejection(label, &bytes);
    }
}

#[test]
fn rejects_every_noncanonical_zip_header_field_before_component_preflight() {
    let base = package_bytes(manifest_toml().as_bytes(), UI_DEMO_COMPONENT).unwrap();
    let locals = signature_offsets(&base, [0x50, 0x4b, 0x03, 0x04]);
    let centrals = signature_offsets(&base, [0x50, 0x4b, 0x01, 0x02]);
    let eocd = signature_offsets(&base, [0x50, 0x4b, 0x05, 0x06])[0];

    let mut cases = Vec::new();
    let mut alternate_flags = base.clone();
    for offset in locals
        .iter()
        .map(|offset| offset + 6)
        .chain(centrals.iter().map(|offset| offset + 8))
    {
        let flags = u16::from_le_bytes(alternate_flags[offset..offset + 2].try_into().unwrap());
        alternate_flags[offset..offset + 2].copy_from_slice(&(flags ^ 0x0004).to_le_bytes());
    }
    cases.push(("alternate flags", alternate_flags));

    for (label, offset, width) in [
        ("local version needed", locals[0] + 4, 2_usize),
        ("central version made by", centrals[0] + 4, 2),
        ("central version needed", centrals[0] + 6, 2),
        ("central internal attributes", centrals[0] + 36, 2),
        ("central low external attributes", centrals[0] + 38, 2),
        ("EOCD disk number", eocd + 4, 2),
        ("EOCD central-directory disk", eocd + 6, 2),
        ("EOCD central-directory size", eocd + 12, 4),
        ("EOCD central-directory offset", eocd + 16, 4),
    ] {
        let mut bytes = base.clone();
        bytes[offset] ^= 1;
        debug_assert!(matches!(width, 2 | 4));
        cases.push((label, bytes));
    }

    let mut pointer_differential = base.clone();
    let first_local = u32::from_le_bytes(
        pointer_differential[centrals[0] + 42..centrals[0] + 46]
            .try_into()
            .unwrap(),
    );
    pointer_differential[centrals[1] + 42..centrals[1] + 46]
        .copy_from_slice(&first_local.to_le_bytes());
    cases.push(("central/local pointer differential", pointer_differential));

    for (label, bytes) in cases {
        assert_archive_preflight_rejection(label, &bytes);
    }
}

#[test]
fn rejects_dishonest_eocd_envelopes_before_ziparchive_allocation() {
    let base = package_bytes(manifest_toml().as_bytes(), UI_DEMO_COMPONENT).unwrap();
    let eocd = signature_offsets(&base, [0x50, 0x4b, 0x05, 0x06])[0];
    let mut cases = Vec::new();

    for (label, at, value) in [
        ("multidisk archive", eocd + 4, 1_u16),
        ("central directory on another disk", eocd + 6, 1),
        ("entries-on-disk mismatch", eocd + 8, 1),
        ("entry-count amplification", eocd + 10, 3),
        ("ZIP64 entry-count sentinel", eocd + 10, u16::MAX),
    ] {
        let mut bytes = base.clone();
        bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
        cases.push((label, bytes));
    }

    for (label, at, value) in [
        ("ZIP64 central-size sentinel", eocd + 12, u32::MAX),
        ("central-size overflow", eocd + 12, u32::MAX - 1),
        ("ZIP64 central-offset sentinel", eocd + 16, u32::MAX),
        ("central-offset out of bounds", eocd + 16, base.len() as u32),
    ] {
        let mut bytes = base.clone();
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        cases.push((label, bytes));
    }

    let mut commented = base.clone();
    commented[eocd + 20..eocd + 22].copy_from_slice(&1_u16.to_le_bytes());
    commented.push(b'x');
    cases.push(("EOCD comment", commented));

    let mut zip64_locator = base.clone();
    zip64_locator.splice(eocd..eocd, b"PK\x06\x07".iter().copied().chain([0_u8; 16]));
    cases.push(("ZIP64 locator", zip64_locator));

    for (label, bytes) in cases {
        assert_envelope_rejection(label, &bytes);
    }
}

#[test]
fn rejects_archive_and_entry_comments_and_extra_fields_before_component_preflight() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.set_comment("hostile-comment");
    for (name, contents) in [
        ("plugin.toml", manifest_toml().as_bytes()),
        ("plugin.wasm", UI_DEMO_COMPONENT),
    ] {
        writer.start_file(name, canonical_options()).unwrap();
        writer.write_all(contents).unwrap();
    }
    let commented = writer.finish().unwrap().into_inner();
    assert_archive_preflight_rejection("archive comment", &commented);

    let base = package_bytes(manifest_toml().as_bytes(), UI_DEMO_COMPONENT).unwrap();
    let central = signature_offsets(&base, [0x50, 0x4b, 0x01, 0x02])[0];
    let eocd = signature_offsets(&base, [0x50, 0x4b, 0x05, 0x06])[0];
    let name_len =
        u16::from_le_bytes(base[central + 28..central + 30].try_into().unwrap()) as usize;

    for (label, len_at) in [
        ("central extra field", central + 30),
        ("entry comment", central + 32),
    ] {
        let mut bytes = base.clone();
        bytes[len_at..len_at + 2].copy_from_slice(&2_u16.to_le_bytes());
        bytes.splice(central + 46 + name_len..central + 46 + name_len, [0_u8, 0]);
        let shifted_eocd = eocd + 2;
        let directory_size = u32::from_le_bytes(
            bytes[shifted_eocd + 12..shifted_eocd + 16]
                .try_into()
                .unwrap(),
        );
        bytes[shifted_eocd + 12..shifted_eocd + 16]
            .copy_from_slice(&(directory_size + 2).to_le_bytes());
        assert_archive_preflight_rejection(label, &bytes);
    }
}

#[test]
fn rejects_package_and_expanded_entry_limits() {
    let oversized_package = vec![0_u8; MAX_WIRT_PACKAGE_BYTES as usize + 1];
    let error = read_package_bytes(&oversized_package)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("archive exceeds"),
        "unexpected rejection: {error}"
    );

    let manifest = vec![b' '; MAX_PLUGIN_MANIFEST_BYTES + 1];
    let oversized_manifest = archive(&[
        ("plugin.toml", &manifest, canonical_options()),
        ("plugin.wasm", b"not a component", canonical_options()),
    ]);
    let error = read_package_bytes(&oversized_manifest)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("archive-bounds: plugin.toml"),
        "unexpected rejection: {error}"
    );

    let component = vec![0_u8; MAX_PLUGIN_WASM_BYTES + 1];
    let oversized_component = archive(&[
        (
            "plugin.toml",
            manifest_toml().as_bytes(),
            canonical_options(),
        ),
        ("plugin.wasm", &component, canonical_options()),
    ]);
    let error = read_package_bytes(&oversized_component)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("archive-bounds: plugin.wasm"),
        "unexpected rejection: {error}"
    );
}

#[test]
fn rejects_dishonest_central_directory_sizes_before_component_validation() {
    let mut bytes = package_with_names("plugin.toml", "plugin.wasm");
    let central = signature_offsets(&bytes, [0x50, 0x4b, 0x01, 0x02])[1];
    let declared = u32::from_le_bytes(bytes[central + 24..central + 28].try_into().unwrap());
    bytes[central + 24..central + 28].copy_from_slice(&(declared - 1).to_le_bytes());

    let error = read_package_bytes(&bytes).unwrap_err();
    let error = error.to_string();
    assert!(
        error.contains("archive-structure:") || error.contains("archive-bounds:"),
        "unexpected rejection: {error}"
    );
}

#[test]
fn rejects_high_ratio_expansion_before_component_validation() {
    let bomb = vec![0_u8; 2 * 1024 * 1024];
    let bytes = archive(&[
        (
            "plugin.toml",
            manifest_toml().as_bytes(),
            canonical_options(),
        ),
        ("plugin.wasm", &bomb, canonical_options()),
    ]);
    let error = read_package_bytes(&bytes).unwrap_err();
    assert!(
        error.to_string().contains("expansion ratio"),
        "unexpected rejection: {error}"
    );
}

#[test]
fn manifest_network_authority_requires_exact_domains() {
    let invalid_manifests = [
        manifest_toml().replace("network = false", "network = true"),
        manifest_toml().replace(
            "network_domains = []",
            "network_domains = [\"api.example.com\"]",
        ),
    ];
    for manifest in invalid_manifests {
        let bytes = archive(&[
            ("plugin.toml", manifest.as_bytes(), canonical_options()),
            ("plugin.wasm", UI_DEMO_COMPONENT, canonical_options()),
        ]);
        assert!(read_package_bytes(&bytes).is_err());
    }
}

#[test]
fn hostile_manifest_values_and_parser_diagnostics_are_bounded() {
    let marker = "HOSTILE-MANIFEST-VALUE".repeat(400);
    let invalid_abi = manifest_toml().replace("abi = \"0.2.0\"", &format!("abi = \"{marker}\""));
    let invalid_domain = manifest_toml()
        .replace("network = false", "network = true")
        .replace(
            "network_domains = []",
            &format!("network_domains = [\"{marker}\"]"),
        );
    let malformed = format!("[wirt]\nabi = \"0.2.0\"\n{marker}");

    for (label, manifest) in [
        ("ABI", invalid_abi),
        ("domain", invalid_domain),
        ("TOML", malformed),
    ] {
        let bytes = archive(&[
            ("plugin.toml", manifest.as_bytes(), canonical_options()),
            ("plugin.wasm", UI_DEMO_COMPONENT, canonical_options()),
        ]);
        let error = read_package_bytes(&bytes).unwrap_err().to_string();
        assert!(
            error.len() <= 240,
            "{label} error was unbounded: {} bytes",
            error.len()
        );
        assert!(
            !error.contains(&marker),
            "{label} error reflected hostile input"
        );
    }
}

#[test]
fn package_refusal_distinguishes_manifest_from_host_mismatch() {
    // A package a version behind the host. The refusal must be the host
    // mismatch alone — with the remedy — not the string that used to
    // report the manifest, the component, and the host as one failure.
    let current = format!("abi = \"{WIRT_ABI_VERSION}\"");
    let stale = manifest_toml().replace(&current, "abi = \"0.1.0\"");
    assert_ne!(
        stale,
        manifest_toml(),
        "the fixture manifest no longer declares {current:?}"
    );
    let bytes = archive(&[
        ("plugin.toml", stale.as_bytes(), canonical_options()),
        ("plugin.wasm", UI_DEMO_COMPONENT, canonical_options()),
    ]);

    let message = read_package_bytes(&bytes).unwrap_err().to_string();
    assert!(
        message.contains("0.1.0"),
        "names the version found: {message}"
    );
    assert!(
        message.contains(WIRT_ABI_VERSION),
        "names the version required: {message}"
    );
    assert!(
        message.contains("wirt-sdk/template"),
        "says where to rebuild: {message}"
    );
    assert!(
        !message.contains("manifest, component, and host"),
        "the three cases are no longer one message: {message}"
    );
}
