use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use std::io::{self, Read, Write};

fn reserve_bounded_capacity(
    body: &mut Vec<u8>,
    required_length: usize,
    limit: usize,
) -> Result<()> {
    if required_length <= body.capacity() {
        return Ok(());
    }

    let doubled_capacity = body.capacity().saturating_mul(2).min(limit);
    let target_capacity = doubled_capacity.max(required_length);
    body.try_reserve_exact(target_capacity - body.len())
        .map_err(|_| anyhow!("failed to allocate bounded resource body"))
}

pub(crate) fn read_to_end_with_limit<R: Read>(
    reader: &mut R,
    limit: usize,
    description: &str,
) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut chunk = [0_u8; 8 * 1024];

    loop {
        let remaining = limit - body.len();
        if remaining == 0 {
            let mut overflow = [0_u8; 1];
            if reader
                .read(&mut overflow)
                .with_context(|| format!("reading {description}"))?
                != 0
            {
                bail!("{description} exceeds the {limit}-byte materialized read limit");
            }
            break;
        }

        let read_length = remaining.min(chunk.len());
        let bytes_read = reader
            .read(&mut chunk[..read_length])
            .with_context(|| format!("reading {description}"))?;
        if bytes_read == 0 {
            break;
        }

        let next_length = body
            .len()
            .checked_add(bytes_read)
            .ok_or_else(|| anyhow!("{description} byte count overflowed"))?;
        reserve_bounded_capacity(&mut body, next_length, limit)?;
        body.extend_from_slice(&chunk[..bytes_read]);
    }

    Ok(body)
}

struct BoundedVecWriter {
    body: Vec<u8>,
    limit: usize,
    description: &'static str,
}

impl BoundedVecWriter {
    fn new(limit: usize, description: &'static str) -> Self {
        Self {
            body: Vec::new(),
            limit,
            description,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.body
    }
}

impl Write for BoundedVecWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let next_length = self.body.len().checked_add(bytes.len()).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "serialized byte count overflowed",
            )
        })?;
        if next_length > self.limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} exceeds the {}-byte materialized read limit",
                    self.description, self.limit
                ),
            ));
        }
        reserve_bounded_capacity(&mut self.body, next_length, self.limit)
            .map_err(io::Error::other)?;
        self.body.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn serialize_json_with_limit<T: Serialize>(
    value: &T,
    limit: usize,
    description: &'static str,
) -> Result<Vec<u8>> {
    let mut writer = BoundedVecWriter::new(limit, description);
    serde_json::to_writer(&mut writer, value)?;
    Ok(writer.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_json_writer_accepts_exact_boundary_and_rejects_next_byte() {
        let expected = serde_json::to_vec("secret").unwrap();
        assert_eq!(
            serialize_json_with_limit(&"secret", expected.len(), "metadata").unwrap(),
            expected
        );
        let error = serialize_json_with_limit(&"secret", expected.len() - 1, "metadata")
            .expect_err("serialization must stop at the limit");
        assert!(error.to_string().contains("materialized read limit"));
    }
}
