//! Limited original-header attestation. Only framing, header integrity, names
//! and entry identity are read here. UnRAR owns all payload decoding and CRCs.
use crate::ImportError;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
};
use unboxer::Limits;
pub(super) struct Header {
    pub name: String,
    pub size: u64,
    pub packed: u64,
    pub host: u32,
    pub attributes: u32,
    pub directory: bool,
}
fn u16_at(b: &[u8], i: usize) -> Result<u16, ImportError> {
    Ok(u16::from_le_bytes(
        b.get(i..i + 2)
            .ok_or(ImportError::Corrupt)?
            .try_into()
            .unwrap(),
    ))
}
fn u32_at(b: &[u8], i: usize) -> Result<u32, ImportError> {
    Ok(u32::from_le_bytes(
        b.get(i..i + 4)
            .ok_or(ImportError::Corrupt)?
            .try_into()
            .unwrap(),
    ))
}
fn var(b: &[u8], cursor: &mut usize) -> Result<u64, ImportError> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *b.get(*cursor).ok_or(ImportError::Corrupt)?;
        *cursor += 1;
        if shift == 63 && byte > 1 {
            return Err(ImportError::Corrupt);
        }
        value |= ((byte & 127) as u64) << shift;
        if byte & 128 == 0 {
            return Ok(value);
        }
    }
    Err(ImportError::Corrupt)
}
fn original_name(bytes: &[u8], ascii: bool) -> Result<String, ImportError> {
    if bytes.len() > 4096 || bytes.contains(&0) || (ascii && !bytes.is_ascii()) {
        return Err(ImportError::Unsupported);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ImportError::Unsupported)?;
    // RAR4's backslash separator is specified by the format. RAR5 backslashes
    // are original filename characters and must never be silently repaired.
    let name = if ascii {
        text.replace('\\', "/")
    } else {
        text.to_owned()
    };
    unboxer::validate_entry_path(&name).map_err(|_| ImportError::Unsupported)?;
    Ok(name)
}

/// A RAR4 name field carrying the UNICODE flag: the name in the archiver's
/// single-byte code page, a NUL, then the Unicode name encoded against it.
///
/// The encoding is the one UnRAR's `DecodeFileName` reads: a high byte,
/// then two-bit codes taken from flag bytes, most significant pair first.
/// Code 0 copies one byte as a code unit, 1 pairs one byte with the high
/// byte, 2 takes two little-endian bytes, and 3 repeats a run of the
/// single-byte name, with a correction added to each byte when the run's
/// length has its top bit set. The decoded name is what UnRAR reports for
/// the entry, so the native header is attested against it afterwards. A
/// stream that runs past its bytes or past the single-byte name, a plain
/// run holding a byte outside ASCII, or a result that is not a valid name
/// refuses the entry rather than guessing what UnRAR would report.
fn unicode_name(field: &[u8]) -> Result<String, ImportError> {
    if field.len() > 4096 {
        return Err(ImportError::Unsupported);
    }
    let nul = field
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(ImportError::Unsupported)?;
    let (single_byte, encoded) = (&field[..nul], &field[nul + 1..]);
    let high = u16::from(*encoded.first().ok_or(ImportError::Unsupported)?) << 8;
    let byte_at = |at: usize| encoded.get(at).copied().ok_or(ImportError::Unsupported);
    let mut units: Vec<u16> = Vec::with_capacity(single_byte.len());
    let mut position = 1;
    let mut flags = 0u8;
    let mut flag_bits = 0;
    while position < encoded.len() {
        if units.len() > 4096 {
            return Err(ImportError::Unsupported);
        }
        if flag_bits == 0 {
            flags = byte_at(position)?;
            position += 1;
            flag_bits = 8;
        }
        match flags >> 6 {
            0 => {
                units.push(u16::from(byte_at(position)?));
                position += 1;
            }
            1 => {
                units.push(u16::from(byte_at(position)?) | high);
                position += 1;
            }
            2 => {
                units.push(u16::from_le_bytes([
                    byte_at(position)?,
                    byte_at(position + 1)?,
                ]));
                position += 2;
            }
            _ => {
                let length = byte_at(position)?;
                position += 1;
                let correction = if length & 0x80 != 0 {
                    position += 1;
                    Some(byte_at(position - 1)?)
                } else {
                    None
                };
                for _ in 0..usize::from(length & 0x7f) + 2 {
                    let byte = *single_byte
                        .get(units.len())
                        .ok_or(ImportError::Unsupported)?;
                    units.push(match correction {
                        Some(correction) => u16::from(byte.wrapping_add(correction)) | high,
                        None if byte.is_ascii() => u16::from(byte),
                        None => return Err(ImportError::Unsupported),
                    });
                }
            }
        }
        flags <<= 2;
        flag_bits -= 2;
    }
    let text = String::from_utf16(&units).map_err(|_| ImportError::Unsupported)?;
    let name = text.replace('\\', "/");
    unboxer::validate_entry_path(&name).map_err(|_| ImportError::Unsupported)?;
    Ok(name)
}

/// The length of a RAR4 extended time record: two flag bytes, then for each
/// of the four times whose top flag bit is set, a four-byte time for all but
/// the modification time (which the header itself carries) and as many extra
/// precision bytes as the low two bits say.
fn extended_time_length(bytes: &[u8]) -> Result<usize, ImportError> {
    let flags = u16_at(bytes, 0)?;
    let mut length = 2;
    for time in 0..4 {
        let mode = flags >> ((3 - time) * 4);
        if mode & 8 == 0 {
            continue;
        }
        if time != 0 {
            length += 4;
        }
        length += usize::from(mode & 3);
    }
    Ok(length)
}

/// The types of the extra records a RAR5 header carries, in order.
fn extra_record_types(area: &[u8]) -> Result<Vec<u64>, ImportError> {
    let mut types = Vec::new();
    let mut cursor = 0;
    while cursor < area.len() {
        let size = usize::try_from(var(area, &mut cursor)?).map_err(|_| ImportError::Limit)?;
        let end = cursor.checked_add(size).ok_or(ImportError::Corrupt)?;
        let record = area.get(cursor..end).ok_or(ImportError::Corrupt)?;
        let mut inner = 0;
        types.push(var(record, &mut inner)?);
        cursor = end;
    }
    Ok(types)
}

/// The packed size a RAR4 file or sub-block header declares.
fn rar4_packed_size(bytes: &[u8], flags: u16) -> Result<u64, ImportError> {
    let mut packed = u32_at(bytes, 7)? as u64;
    if flags & 0x100 != 0 {
        packed |= (u32_at(bytes, 32)? as u64) << 32;
    }
    Ok(packed)
}
pub(super) fn attest(file: &mut File, limits: &Limits) -> Result<Vec<Header>, ImportError> {
    file.seek(SeekFrom::Start(0))?;
    let size = file.metadata()?.len();
    let mut signature = [0; 8];
    file.read_exact(&mut signature)?;
    let version = if signature == *b"Rar!\x1a\x07\x01\x00" {
        5
    } else if signature[..7] == *b"Rar!\x1a\x07\x00" {
        4
    } else {
        return Err(ImportError::Unsupported);
    };
    let mut offset = if version == 5 { 8 } else { 7 };
    let mut headers = vec![];
    let mut total = 0u64;
    let mut main = false;
    let mut ended = false;
    while offset < size {
        file.seek(SeekFrom::Start(offset))?;
        if version == 4 {
            let mut prefix = [0; 7];
            file.read_exact(&mut prefix)?;
            let length = u16_at(&prefix, 5)? as usize;
            if length < 7 {
                return Err(ImportError::Corrupt);
            }
            total += length as u64;
            if total > limits.max_index_bytes {
                return Err(ImportError::Limit);
            }
            let mut bytes = vec![0; length];
            bytes[..7].copy_from_slice(&prefix);
            file.read_exact(&mut bytes[7..])?;
            if crc32fast::hash(&bytes[2..]) as u16 != u16_at(&bytes, 0)? {
                return Err(ImportError::Corrupt);
            }
            let flags = u16_at(&bytes, 3)?;
            let mut packed = 0u64;
            match bytes[2] {
                0x73 => {
                    // Encrypted headers are reported as such. Solid, locked
                    // and protected (a recovery record follows) archives
                    // hold the same entries as plain ones; volumes and the
                    // old in-header comment are refused.
                    if flags & 0x0080 != 0 {
                        return Err(ImportError::Encrypted);
                    }
                    if main
                        || !headers.is_empty()
                        || flags & !(0x0004 | 0x0008 | 0x0040) != 0
                        || length != 13
                    {
                        return Err(ImportError::Unsupported);
                    }
                    main = true;
                }
                0x7a => {
                    // A sub-block (a comment, a recovery record, an NTFS
                    // stream) has a file header's shape and is not an
                    // entry; UnRAR lists none of them. It is skipped with
                    // its data, unless it is encrypted.
                    if flags & 0x0004 != 0 {
                        return Err(ImportError::Encrypted);
                    }
                    if !main || flags & 0x8000 == 0 || length < 32 {
                        return Err(ImportError::Unsupported);
                    }
                    packed = rar4_packed_size(&bytes, flags)?;
                }
                0x74 => {
                    // Long block, large sizes, solid, the dictionary bits,
                    // an extended time record and a Unicode name are the
                    // shapes WinRAR writes; a password is reported as such.
                    if flags & 0x0004 != 0 {
                        return Err(ImportError::Encrypted);
                    }
                    if !main
                        || flags & !(0x8000 | 0x0100 | 0x0010 | 0x00e0 | 0x1000 | 0x0200) != 0
                        || flags & 0x8000 == 0
                        || length < 32
                    {
                        return Err(ImportError::Unsupported);
                    }
                    packed = rar4_packed_size(&bytes, flags)?;
                    let mut unpacked = u32_at(&bytes, 11)? as u64;
                    let name_len = u16_at(&bytes, 26)? as usize;
                    let mut start: usize = 32;
                    if flags & 0x100 != 0 {
                        unpacked |= (u32_at(&bytes, 36)? as u64) << 32;
                        start = 40;
                    }
                    let name_end = start
                        .checked_add(name_len)
                        .filter(|end| *end <= length)
                        .ok_or(ImportError::Unsupported)?;
                    let trailing = if flags & 0x1000 != 0 {
                        extended_time_length(&bytes[name_end..])?
                    } else {
                        0
                    };
                    if name_end + trailing != length {
                        return Err(ImportError::Unsupported);
                    }
                    let raw_host = bytes[15];
                    // Native UnRAR maps unknown host values to a known host and
                    // invents attributes. Accept only the original schemes we
                    // can attest (DOS, Windows, Unix), with modern RAR4 flags.
                    let host = match raw_host {
                        0 | 2 => 2,
                        3 => 3,
                        _ => return Err(ImportError::Unsupported),
                    };
                    if bytes[24] < 20 {
                        return Err(ImportError::Unsupported);
                    }
                    let attributes = u32_at(&bytes, 28)?;
                    let directory = flags & 0xe0 == 0xe0;
                    let name = if flags & 0x0200 != 0 {
                        unicode_name(&bytes[start..name_end])?
                    } else {
                        original_name(&bytes[start..name_end], true)?
                    };
                    headers.push(Header {
                        name,
                        size: unpacked,
                        packed,
                        host,
                        attributes,
                        directory,
                    });
                }
                0x7b => {
                    // WinRAR marks the end header skippable (0x4000) and may
                    // append the CRC of the archive's data (2); a next-volume
                    // or volume-number mark is a volume, which is refused.
                    let data_crc = usize::from(flags & 0x0002 != 0) * 4;
                    if !main || flags & !(0x4000 | 0x0002) != 0 || length != 7 + data_crc {
                        return Err(ImportError::Unsupported);
                    }
                    ended = true;
                }
                _ => return Err(ImportError::Unsupported),
            }
            offset = offset
                .checked_add(length as u64)
                .and_then(|n| n.checked_add(packed))
                .ok_or(ImportError::Corrupt)?;
        } else {
            let mut crc = [0; 4];
            file.read_exact(&mut crc)?;
            let mut encoded = Vec::with_capacity(10);
            for _ in 0..10 {
                let mut byte = [0];
                file.read_exact(&mut byte)?;
                encoded.push(byte[0]);
                if byte[0] & 128 == 0 {
                    break;
                }
            }
            let mut cursor = 0;
            let length = var(&encoded, &mut cursor)?;
            if cursor != encoded.len() || length > 1 << 20 {
                return Err(ImportError::Limit);
            }
            total = total
                .checked_add(length + encoded.len() as u64 + 4)
                .ok_or(ImportError::Limit)?;
            if total > limits.max_index_bytes {
                return Err(ImportError::Limit);
            }
            let mut bytes = vec![0; length as usize];
            file.read_exact(&mut bytes)?;
            let mut hash = crc32fast::Hasher::new();
            hash.update(&encoded);
            hash.update(&bytes);
            if hash.finalize() != u32::from_le_bytes(crc) {
                return Err(ImportError::Corrupt);
            }
            let mut cursor = 0;
            let kind = var(&bytes, &mut cursor)?;
            let flags = var(&bytes, &mut cursor)?;
            // An extra area (1), a data area (2) and the skip-if-unknown
            // mark (4) are what WinRAR writes on every archive; the split
            // and child flags are the volume shapes this reader refuses.
            if flags & !7 != 0 {
                return Err(ImportError::Unsupported);
            }
            let extra = if flags & 1 != 0 {
                usize::try_from(var(&bytes, &mut cursor)?).map_err(|_| ImportError::Limit)?
            } else {
                0
            };
            let packed = if flags & 2 != 0 {
                var(&bytes, &mut cursor)?
            } else {
                0
            };
            let body_end = bytes
                .len()
                .checked_sub(extra)
                .filter(|end| *end >= cursor)
                .ok_or(ImportError::Corrupt)?;
            let extra_area = &bytes[body_end..];
            match kind {
                1 => {
                    // Solid (4), a recovery record present (8) and locked
                    // (16) archives hold the same entries as plain ones;
                    // volumes are refused. The extra area holds the locator
                    // (1), which positions the quick-open and recovery
                    // records, and the archive's own name and time (2).
                    let archive_flags = var(&bytes, &mut cursor)?;
                    if main || flags & 2 != 0 || archive_flags & !(4 | 8 | 16) != 0 {
                        return Err(ImportError::Unsupported);
                    }
                    for record in extra_record_types(extra_area)? {
                        if !matches!(record, 1 | 2) {
                            return Err(ImportError::Unsupported);
                        }
                    }
                    main = true;
                }
                2 | 3 => {
                    if !main {
                        return Err(ImportError::Corrupt);
                    }
                    // A service header (3) has a file header's shape and is
                    // not an entry: the quick-open, comment and recovery
                    // records it carries are skipped with their data, and
                    // UnRAR lists none of them.
                    let file_flags = var(&bytes, &mut cursor)?;
                    if file_flags & !7 != 0 {
                        return Err(ImportError::Unsupported);
                    }
                    let unpacked = var(&bytes, &mut cursor)?;
                    let attributes = u32::try_from(var(&bytes, &mut cursor)?)
                        .map_err(|_| ImportError::Unsupported)?;
                    if file_flags & 2 != 0 {
                        u32_at(&bytes, cursor)?;
                        cursor += 4;
                    }
                    if file_flags & 4 != 0 {
                        u32_at(&bytes, cursor)?;
                        cursor += 4;
                    }
                    let _compression = var(&bytes, &mut cursor)?;
                    let host = var(&bytes, &mut cursor)?;
                    if host > 1 {
                        return Err(ImportError::Unsupported);
                    }
                    let name_len = usize::try_from(var(&bytes, &mut cursor)?)
                        .map_err(|_| ImportError::Limit)?;
                    let end = cursor.checked_add(name_len).ok_or(ImportError::Limit)?;
                    let name_bytes = bytes.get(cursor..end).ok_or(ImportError::Corrupt)?;
                    cursor = end;
                    // File time (3), hash (2) and unix owner (6) records
                    // describe an entry without changing what it is.
                    // Encryption (1) is reported as such; a version (4), a
                    // redirection (5) or service data (7) on an entry
                    // changes its name or the object it is, and is refused.
                    for record in extra_record_types(extra_area)? {
                        match record {
                            1 => return Err(ImportError::Encrypted),
                            _ if kind == 3 => {}
                            2 | 3 | 6 => {}
                            _ => return Err(ImportError::Unsupported),
                        }
                    }
                    if kind == 2 {
                        headers.push(Header {
                            name: original_name(name_bytes, false)?,
                            size: unpacked,
                            packed,
                            host: if host == 0 { 2 } else { 3 },
                            attributes,
                            directory: file_flags & 1 != 0,
                        });
                    }
                }
                4 => return Err(ImportError::Encrypted),
                5 => {
                    if !main || flags & 3 != 0 || var(&bytes, &mut cursor)? != 0 {
                        return Err(ImportError::Unsupported);
                    }
                    ended = true;
                }
                _ => return Err(ImportError::Unsupported),
            }
            if cursor != body_end {
                return Err(ImportError::Unsupported);
            }
            offset = offset
                .checked_add(4 + encoded.len() as u64 + length)
                .and_then(|n| n.checked_add(packed))
                .ok_or(ImportError::Corrupt)?;
        }
        if headers.len() as u64 > limits.max_entries {
            return Err(ImportError::Limit);
        }
        if offset > size || ended && offset != size {
            return Err(ImportError::Corrupt);
        }
        if ended {
            break;
        }
    }
    if !ended {
        return Err(ImportError::Corrupt);
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(headers)
}
