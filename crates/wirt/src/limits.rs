pub const MAX_PLUGIN_METADATA_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PLUGIN_GUEST_DATA_BYTES: usize = 4 * 1024 * 1024;

pub fn metadata_value_within_limit(value: &serde_json::Value) -> bool {
    struct LimitWriter {
        written: usize,
    }

    impl std::io::Write for LimitWriter {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            if self.written.saturating_add(buffer.len()) > MAX_PLUGIN_METADATA_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "metadata publication limit exceeded",
                ));
            }
            self.written += buffer.len();
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    serde_json::to_writer(&mut LimitWriter { written: 0 }, value).is_ok()
}
