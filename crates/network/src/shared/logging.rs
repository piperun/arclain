use sha2::{Digest, Sha256};
use std::fmt;

const DISPLAY_DIGEST_BYTES: usize = 12;

pub(crate) struct SafeLogFingerprint {
    digest: [u8; DISPLAY_DIGEST_BYTES],
    byte_len: usize,
}

/// Stable, non-reversible identifier for network values that must not reach
/// logs verbatim, including URLs, domains, plugin IDs, and transport errors.
pub(crate) fn safe_log_fingerprint(value: impl AsRef<[u8]>) -> SafeLogFingerprint {
    let value = value.as_ref();
    let full_digest = Sha256::digest(value);
    let mut digest = [0_u8; DISPLAY_DIGEST_BYTES];
    digest.copy_from_slice(&full_digest[..DISPLAY_DIGEST_BYTES]);
    SafeLogFingerprint {
        digest,
        byte_len: value.len(),
    }
}

impl fmt::Display for SafeLogFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("sha256:")?;
        for byte in self.digest {
            write!(formatter, "{byte:02x}")?;
        }
        write!(formatter, " bytes={}", self.byte_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_never_echoes_network_values_or_control_characters() {
        let marker = "https://secret.test/?token=abc\r\nforged-event";

        let first = safe_log_fingerprint(marker).to_string();
        let second = safe_log_fingerprint(marker).to_string();

        assert_eq!(first, second);
        assert!(first.starts_with("sha256:"));
        assert!(first.ends_with(&format!(" bytes={}", marker.len())));
        assert!(!first.contains(marker));
        assert!(!first.contains('\r'));
        assert!(!first.contains('\n'));
    }
}
