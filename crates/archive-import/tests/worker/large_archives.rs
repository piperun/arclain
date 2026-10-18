//! Worker admission and decoding keep encoded input and retained paths bounded.
use super::*;
use arclain_archive::ImportError;
use std::io::Write;

fn push16(out: &mut Vec<u8>, value: u16) { out.extend_from_slice(&value.to_le_bytes()); }
fn push32(out: &mut Vec<u8>, value: u32) { out.extend_from_slice(&value.to_le_bytes()); }
fn push64(out: &mut Vec<u8>, value: u64) { out.extend_from_slice(&value.to_le_bytes()); }

// Stored one-byte files. ZIP64 end records preserve counts above65,535.
fn write_zip(path: &std::path::Path, names: impl Iterator<Item = String>) {
    let mut out = std::fs::File::create(path).unwrap();
    let mut central = Vec::new();
    let mut offset = 0u32;
    let mut count = 0u64;
    let crc = crc32fast::hash(b"x");
    for name in names {
        let name = name.as_bytes();
        let mut local = Vec::new();
        push32(&mut local, 0x0403_4b50);
        for value in [20, 0x800, 0, 0, 0] { push16(&mut local, value); }
        push32(&mut local, crc);
        push32(&mut local, 1); push32(&mut local, 1);
        push16(&mut local, name.len() as u16); push16(&mut local, 0);
        local.extend_from_slice(name); local.push(b'x');
        out.write_all(&local).unwrap();
        push32(&mut central, 0x0201_4b50);
        for value in [(3 << 8) | 20, 20, 0x800, 0, 0, 0] { push16(&mut central, value); }
        push32(&mut central, crc);
        push32(&mut central, 1); push32(&mut central, 1);
        push16(&mut central, name.len() as u16);
        for _ in 0..4 { push16(&mut central, 0); }
        push32(&mut central, 0o100644 << 16); push32(&mut central, offset);
        central.extend_from_slice(name);
        offset += local.len() as u32;
        count += 1;
    }
    out.write_all(&central).unwrap();
    let mut end = Vec::new();
    if count >= u64::from(u16::MAX) {
        push32(&mut end, 0x0606_4b50); push64(&mut end, 44);
        push16(&mut end, 45); push16(&mut end, 45);
        push32(&mut end, 0); push32(&mut end, 0);
        push64(&mut end, count); push64(&mut end, count);
        push64(&mut end, central.len() as u64); push64(&mut end, offset.into());
        push32(&mut end, 0x0706_4b50); push32(&mut end, 0);
        push64(&mut end, u64::from(offset) + central.len() as u64); push32(&mut end, 1);
    }
    push32(&mut end, 0x0605_4b50); push16(&mut end, 0); push16(&mut end, 0);
    let short_count = u16::try_from(count).unwrap_or(u16::MAX);
    push16(&mut end, short_count); push16(&mut end, short_count);
    push32(&mut end, central.len() as u32); push32(&mut end, offset); push16(&mut end, 0);
    out.write_all(&end).unwrap();
}

fn large_limits() -> unboxer::Limits { unboxer::Limits::new(200_000, 1 << 20, 200, 64 << 20) }

#[test]
fn large_inventory_decodes_with_read_budget_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.zip");
    write_zip(&path, (0..50_000).map(|i| format!("Game/data/{i:06}.dat")));
    let mut session = ArchiveClient::new(options()).open(&path, large_limits(), Cancellation::default()).unwrap();
    let mut count = 0;
    while let Some(mut entry) = session.next_entry().unwrap() {
        let mut bytes = Vec::new();
        entry.reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"x");
        count += 1;
    }
    assert_eq!(count, 50_000);
}

#[test]
fn too_many_entries_is_a_limit_not_corruption() {
    let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("count.zip");
    write_zip(&path, (0..200_001).map(|i| format!("{i:06}.dat")));
    let error = ArchiveClient::new(options()).open(&path, large_limits(), Cancellation::default()).err();
    assert_eq!(error, Some(ImportError::Limit));
}

#[test]
fn deep_paths_are_refused_by_worker_before_first_stream() {
    let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("deep.zip");
    let tail = "a/".repeat(30);
    write_zip(&path, (0..20_000).map(|i| format!("{i:05}/{tail}x")));
    let mut session = ArchiveClient::new(options()).open(&path, large_limits(), Cancellation::default()).unwrap();
    assert!(session.next_entry().is_err());
    assert_eq!(session.take_error(), Some(ImportError::Limit));
}

#[test]
fn smaller_caller_index_budget_still_bounds_encoded_input() {
    let mut bounds = limits(); bounds.max_index_bytes = 1;
    let error = ArchiveClient::new(options()).open(&fixture("game.zip"), bounds, Cancellation::default()).err();
    assert_eq!(error, Some(ImportError::Limit));
}

#[test]
fn caller_index_budget_is_not_the_worker_path_budget() {
    let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("small.zip");
    write_zip(&path, (0..1_000).map(|i| format!("Game/data/{i:06}.dat")));
    let mut bounds = large_limits(); bounds.max_index_bytes = 1 << 20;
    let mut session = ArchiveClient::new(options()).open(&path, bounds, Cancellation::default()).unwrap();
    assert!(inspect_inventory(session.inventory(), &bounds).is_err());
    let mut count = 0;
    while let Some(mut entry) = session.next_entry().unwrap() {
        assert_eq!(std::io::copy(&mut entry.reader, &mut std::io::sink()).unwrap(), 1);
        count += 1;
    }
    assert_eq!(count, 1_000);
}

#[test]
fn larger_encoded_index_requests_remain_refused() {
    let mut bounds = limits(); bounds.max_index_bytes = (64 << 20) + 1;
    let error = ArchiveClient::new(options()).open(&fixture("game.zip"), bounds, Cancellation::default()).err();
    assert_eq!(error, Some(ImportError::Limit));
}
