use crate::ImportError;
use std::{fs::File, io::Write};
use unboxer::{
    ArchiveReader, Container, EntryName, Inventory, Limits, ObjectKind, PackedBlock, PhysicalEntry,
    ZipReader,
};
pub(super) struct Archive {
    reader: ZipReader,
    pub inventory: Inventory,
}
impl Archive {
    pub fn open(file: File, limits: &Limits) -> Result<Self, ImportError> {
        let mut reader = ZipReader::open_file(file, limits)
            .map_err(|error| super::limit_or(error, ImportError::Corrupt))?;
        let mut inventory = Inventory {
            container: Container::Zip,
            entries: vec![],
            blocks: vec![],
        };
        for index in 0..reader.len() {
            let header = reader.header(index).map_err(|_| ImportError::Corrupt)?;
            let kind = if header.is_link {
                ObjectKind::Link
            } else if header
                .unix_mode
                .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o040000 | 0o100000))
            {
                ObjectKind::Special
            } else if header.is_dir {
                ObjectKind::Directory
            } else {
                ObjectKind::File
            };
            let block = if header.is_dir {
                None
            } else {
                let id = inventory.blocks.len();
                inventory.blocks.push(PackedBlock {
                    packed_bytes: header.compressed_size,
                    unpacked_bytes: header.size,
                });
                Some(id)
            };
            inventory.entries.push(PhysicalEntry {
                index,
                name: EntryName::LegacyZip(header.raw_name),
                kind,
                size: header.size,
                block,
                encrypted: header.encrypted,
                method: format!("ZIP:{}", header.method),
                method_supported: header.method_supported,
                unix_mode: header.unix_mode,
            });
        }
        Ok(Self { reader, inventory })
    }
    pub fn decode(&mut self, out: &mut dyn Write) -> Result<(), ImportError> {
        for entry in &self.inventory.entries {
            let mut stream = self
                .reader
                .entry(entry.index)
                .map_err(|_| ImportError::Corrupt)?;
            super::emit(entry.index, entry.size, &mut *stream, out)?;
        }
        Ok(())
    }
}
