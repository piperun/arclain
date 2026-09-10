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
                    if main || !headers.is_empty() || flags & !0x0008 != 0 || length != 13 {
                        return Err(ImportError::Unsupported);
                    }
                    main = true;
                }
                0x74 => {
                    if !main
                        || flags & !(0x8000 | 0x0100 | 0x0010 | 0x00e0) != 0
                        || flags & 0x8000 == 0
                        || length < 32
                    {
                        return Err(ImportError::Unsupported);
                    }
                    packed = u32_at(&bytes, 7)? as u64;
                    let mut unpacked = u32_at(&bytes, 11)? as u64;
                    let name_len = u16_at(&bytes, 26)? as usize;
                    let mut start = 32;
                    if flags & 0x100 != 0 {
                        packed |= (u32_at(&bytes, 32)? as u64) << 32;
                        unpacked |= (u32_at(&bytes, 36)? as u64) << 32;
                        start = 40;
                    }
                    if start + name_len != length {
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
                    let name = original_name(&bytes[start..], true)?;
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
                    if !main || flags != 0 || length != 7 {
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
            // Extra/service/header semantics can rewrite names or represent
            // links. This first subset accepts neither, even if native UnRAR
            // could silently skip or normalize them.
            if flags & !2 != 0 {
                return Err(ImportError::Unsupported);
            }
            let packed = if flags & 2 != 0 {
                var(&bytes, &mut cursor)?
            } else {
                0
            };
            match kind {
                1 => {
                    if main || flags != 0 || var(&bytes, &mut cursor)? != 0 {
                        return Err(ImportError::Unsupported);
                    }
                    main = true;
                }
                2 => {
                    if !main {
                        return Err(ImportError::Corrupt);
                    }
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
                    let name =
                        original_name(bytes.get(cursor..end).ok_or(ImportError::Corrupt)?, false)?;
                    cursor = end;
                    headers.push(Header {
                        name,
                        size: unpacked,
                        packed,
                        host: if host == 0 { 2 } else { 3 },
                        attributes,
                        directory: file_flags & 1 != 0,
                    });
                }
                5 => {
                    if !main || flags != 0 || var(&bytes, &mut cursor)? != 0 {
                        return Err(ImportError::Unsupported);
                    }
                    ended = true;
                }
                _ => return Err(ImportError::Unsupported),
            }
            if cursor != bytes.len() {
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
