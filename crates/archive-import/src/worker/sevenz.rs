use crate::ImportError;
use sevenz_rust2::{Archive as Metadata, BlockDecoder, EncoderMethod, Password};
use std::{fs::File, io::Write};
use unboxer::{Container, EntryName, Inventory, Limits, ObjectKind, PackedBlock, PhysicalEntry};
pub(super) struct Archive {
    file: File,
    metadata: Metadata,
    physical_indices: Vec<usize>,
    pub inventory: Inventory,
}
impl Archive {
    pub fn open(mut file: File, limits: &Limits) -> Result<Self, ImportError> {
        let mut metadata =
            Metadata::read(&mut file, &Password::empty()).map_err(|_| ImportError::Corrupt)?;
        if metadata.files.len() as u64 > limits.max_entries {
            return Err(ImportError::Limit);
        }
        if metadata.blocks.len() > metadata.files.len() {
            return Err(ImportError::Corrupt);
        }
        let mut inventory = Inventory {
            container: Container::SevenZ,
            entries: vec![],
            blocks: vec![],
        };
        let first_packs = metadata.stream_map.block_first_pack_stream_index();
        if first_packs.len() != metadata.blocks.len()
            || metadata.stream_map.file_block_index.len() != metadata.files.len()
        {
            return Err(ImportError::Corrupt);
        }
        for (index, block) in metadata.blocks.iter().enumerate() {
            let start = first_packs[index];
            let end = first_packs
                .get(index + 1)
                .copied()
                .unwrap_or(metadata.pack_sizes().len());
            let packed = metadata
                .pack_sizes()
                .get(start..end)
                .ok_or(ImportError::Corrupt)?
                .iter()
                .try_fold(0u64, |sum, n| {
                    sum.checked_add(*n).ok_or(ImportError::Corrupt)
                })?;
            inventory.blocks.push(PackedBlock {
                packed_bytes: packed,
                unpacked_bytes: block.get_unpack_size(),
            });
        }
        for (index, file) in metadata.files.iter().enumerate() {
            let attributes = file
                .has_windows_attributes
                .then_some(file.windows_attributes);
            let mode = attributes.map(|a| a >> 16).filter(|m| *m != 0);
            let kind = if file.is_anti_item {
                ObjectKind::Special
            } else if attributes.is_some_and(|a| a & 0x400 != 0)
                || mode.is_some_and(|m| m & 0o170000 == 0o120000)
            {
                ObjectKind::Link
            } else if mode.is_some_and(|m| !matches!(m & 0o170000, 0 | 0o040000 | 0o100000)) {
                ObjectKind::Special
            } else if file.is_directory {
                ObjectKind::Directory
            } else {
                ObjectKind::File
            };
            let block = if file.has_stream {
                metadata.stream_map.file_block_index[index]
            } else {
                None
            };
            let coders = match block {
                Some(id) => &metadata.blocks.get(id).ok_or(ImportError::Corrupt)?.coders[..],
                None => &[],
            };
            let encrypted = coders
                .iter()
                .any(|c| c.encoder_method_id() == EncoderMethod::ID_AES256_SHA256);
            let supported = coders.iter().all(|c| {
                matches!(
                    c.encoder_method_id(),
                    [0] | [0x21]
                        | [3, 1, 1]
                        | [3, 3, 1, 3]
                        | [3, 3, 1, 0x1b]
                        | [3, 3, 2, 5]
                        | [3, 3, 4, 1]
                        | [3, 3, 5, 1]
                        | [3, 3, 7, 1]
                        | [3, 3, 8, 5]
                        | [3, 2]
                        | [0xa]
                        | [0xb]
                        | [4, 2, 2]
                        | [3, 4, 1]
                        | [4, 1, 8]
                )
            });
            let method = coders
                .iter()
                .map(|c| {
                    EncoderMethod::by_id(c.encoder_method_id())
                        .map(|m| m.name())
                        .unwrap_or("unknown")
                })
                .collect::<Vec<_>>()
                .join("+");
            inventory.entries.push(PhysicalEntry {
                index,
                name: EntryName::Unicode(file.name.clone()),
                kind,
                size: file.size,
                block,
                encrypted,
                method,
                method_supported: supported,
                unix_mode: mode,
            });
        }
        // BlockDecoder expects a contiguous span of DATA substreams. Physical
        // metadata may interleave empty files/directories, so give the decoder
        // a compact data-only view while keeping original identities separate.
        let mut physical_indices = Vec::new();
        let mut starts = vec![None; metadata.blocks.len()];
        let mut compact = Vec::new();
        let mut previous = None;
        for (index, entry) in std::mem::take(&mut metadata.files).into_iter().enumerate() {
            if !entry.has_stream {
                continue;
            }
            let block = metadata.stream_map.file_block_index[index].ok_or(ImportError::Corrupt)?;
            if previous.is_some_and(|prior| block < prior) {
                return Err(ImportError::Corrupt);
            }
            let start = starts.get_mut(block).ok_or(ImportError::Corrupt)?;
            if start.is_none() {
                *start = Some(compact.len());
            }
            previous = Some(block);
            physical_indices.push(index);
            compact.push(entry);
        }
        metadata.files = compact;
        metadata.stream_map.block_first_file_index = starts
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or(ImportError::Corrupt)?;
        metadata.stream_map.file_block_index = physical_indices
            .iter()
            .map(|i| inventory.entries[*i].block)
            .collect();
        for block in 0..metadata.blocks.len() {
            let start = metadata.stream_map.block_first_file_index[block];
            let end = metadata
                .stream_map
                .block_first_file_index
                .get(block + 1)
                .copied()
                .unwrap_or(metadata.files.len());
            if end - start
                != BlockDecoder::new(1, block, &metadata, &Password::empty(), &mut file)
                    .entry_count()
            {
                return Err(ImportError::Corrupt);
            }
        }
        Ok(Self {
            file,
            metadata,
            physical_indices,
            inventory,
        })
    }
    pub fn decode(&mut self, out: &mut dyn Write) -> Result<(), ImportError> {
        let mut seen = vec![false; self.inventory.entries.len()];
        let password = Password::empty();
        for block in 0..self.metadata.blocks.len() {
            let start = *self
                .metadata
                .stream_map
                .block_first_file_index
                .get(block)
                .ok_or(ImportError::Corrupt)?;
            let mut ordinal = 0usize;
            let mut failure = None;
            BlockDecoder::new(1, block, &self.metadata, &password, &mut self.file)
                .for_each_entries(&mut |entry, reader| {
                    let index = self
                        .physical_indices
                        .get(start + ordinal)
                        .copied()
                        .unwrap_or(usize::MAX);
                    ordinal += 1;
                    let result = if index >= seen.len() || seen[index] {
                        Err(ImportError::Corrupt)
                    } else {
                        seen[index] = true;
                        super::emit(index, entry.size, reader, out)
                    };
                    if let Err(error) = result {
                        failure = Some(error);
                        return Err(sevenz_rust2::Error::Other(std::borrow::Cow::Borrowed(
                            "archive import aborted",
                        )));
                    }
                    Ok(true)
                })
                .map_err(|_| failure.unwrap_or(ImportError::Corrupt))?;
        }
        for (index, entry) in self.inventory.entries.iter().enumerate() {
            if !seen[index] && entry.block.is_none() {
                super::emit(index, entry.size, &mut std::io::empty(), out)?;
                seen[index] = true;
            }
        }
        if seen.iter().any(|s| !*s) {
            return Err(ImportError::Corrupt);
        }
        Ok(())
    }
}
