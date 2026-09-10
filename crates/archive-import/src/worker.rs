//! Narrow resource-bounded decoder entrypoint. No destination paths are accepted.
mod rar;
mod rar_headers;
mod resource_limits;
mod sevenz;
mod source;
mod zip;
use crate::protocol::*;
use std::io::{Read, Write};
use unboxer::{Inventory, Limits};

pub fn main() -> i32 {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let memory = match arguments.as_slice() {
        [flag, number] if flag == "--memory-limit" => number.parse::<u64>().ok(),
        _ => None,
    };
    let Some(memory) = memory.filter(|n| (32 << 20..=4u64 << 30).contains(n)) else {
        return 2;
    };
    // No archive path or request is read before this succeeds.
    let _resources = match resource_limits::establish(memory) {
        Ok(guard) => guard,
        Err(_) => {
            let _ = json(
                &mut std::io::stdout().lock(),
                FAILURE,
                &ImportError::ResourceLimit,
            );
            return 3;
        }
    };
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let result = run(&mut input, &mut output);
    if let Err(error) = result {
        let _ = json(&mut output, FAILURE, &error);
        return 1;
    }
    0
}

fn run(input: &mut dyn Read, output: &mut dyn Write) -> Result<(), ImportError> {
    frame(output, READY, b"arclain-import-v1")?;
    let (tag, data) = read_frame(input)?;
    if tag != OPEN {
        return Err(ImportError::Corrupt);
    }
    let request: Request = parse(&data)?;
    let limits = request.limits.unpack();
    if limits.max_entries > 200_000 || limits.max_index_bytes > 64 << 20 {
        return Err(ImportError::Limit);
    }
    let mut source = source::Source::open(&request.source, limits.max_archive_bytes)?;
    let magic = source.magic()?;
    let mut archive = if magic.starts_with(b"PK\x03\x04") || magic.starts_with(b"PK\x05\x06") {
        Backend::Zip(zip::Archive::open(source.file.try_clone()?, &limits)?)
    } else if magic.starts_with(b"7z\xBC\xAF\x27\x1C") {
        Backend::SevenZ(Box::new(sevenz::Archive::open(
            source.file.try_clone()?,
            &limits,
        )?))
    } else if magic.starts_with(b"Rar!\x1A\x07") {
        Backend::Rar(rar::Archive::open(&mut source, &limits)?)
    } else {
        return Err(ImportError::Unsupported);
    };
    let inventory = archive.inventory();
    send_inventory(output, inventory, &limits)?;
    let (tag, data) = read_frame(input)?;
    if tag != DECODE || !data.is_empty() {
        return Err(ImportError::Corrupt);
    }
    // Refuse unsafe inventory independently of the caller too; this does not
    // confer write authority, and no output path exists in this process.
    unboxer::inspect_inventory(archive.inventory(), &limits)
        .map_err(|_| ImportError::Unsupported)?;
    archive.decode(output)?;
    source.verify()?;
    frame(output, COMPLETE, &[])?;
    Ok(())
}

enum Backend {
    Zip(zip::Archive),
    SevenZ(Box<sevenz::Archive>),
    Rar(rar::Archive),
}
impl Backend {
    fn inventory(&self) -> &Inventory {
        match self {
            Self::Zip(a) => &a.inventory,
            Self::SevenZ(a) => &a.inventory,
            Self::Rar(a) => &a.inventory,
        }
    }
    fn decode(&mut self, out: &mut dyn Write) -> Result<(), ImportError> {
        match self {
            Self::Zip(a) => a.decode(out),
            Self::SevenZ(a) => a.decode(out),
            Self::Rar(a) => a.decode(out),
        }
    }
}
fn send_inventory(
    out: &mut dyn Write,
    inventory: &Inventory,
    limits: &Limits,
) -> Result<(), ImportError> {
    if inventory.entries.len() as u64 > limits.max_entries
        || inventory.blocks.len() > inventory.entries.len()
    {
        return Err(ImportError::Limit);
    }
    json(out, ARCHIVE, &inventory.container)?;
    let mut bytes = 0u64;
    for block in &inventory.blocks {
        let payload = serde_json::to_vec(block).map_err(|_| ImportError::Corrupt)?;
        bytes += payload.len() as u64;
        if bytes > limits.max_index_bytes {
            return Err(ImportError::Limit);
        }
        frame(out, BLOCK, &payload)?;
    }
    for entry in &inventory.entries {
        let payload = serde_json::to_vec(entry).map_err(|_| ImportError::Corrupt)?;
        bytes += payload.len() as u64;
        if bytes > limits.max_index_bytes {
            return Err(ImportError::Limit);
        }
        frame(out, ENTRY, &payload)?;
    }
    frame(
        out,
        INVENTORY_END,
        &(inventory.entries.len() as u64).to_le_bytes(),
    )
}
pub(super) fn emit(
    index: usize,
    declared: u64,
    reader: &mut dyn Read,
    out: &mut dyn Write,
) -> Result<(), ImportError> {
    frame(out, BEGIN, &(index as u64).to_le_bytes())?;
    let mut buffer = [0; MAX_FRAME];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buffer).map_err(|_| ImportError::Corrupt)?;
        if n == 0 {
            break;
        }
        total = total.checked_add(n as u64).ok_or(ImportError::Limit)?;
        if total > declared {
            return Err(ImportError::Corrupt);
        }
        frame(out, DATA, &buffer[..n])?;
    }
    if total != declared {
        return Err(ImportError::Corrupt);
    }
    frame(out, END, &(index as u64).to_le_bytes())
}
