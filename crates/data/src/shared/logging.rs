use sha2::{Digest, Sha256};
use std::fmt;

const DISPLAY_DIGEST_BYTES: usize = 12;

/// Stable, non-reversible identifier for values that must not reach logs.
///
/// Keys, URLs, paths, validators, response bodies, and error chains can all
/// contain user or plugin-controlled secrets. Log this value instead of the
/// original string so events remain correlatable without disclosure.
pub(crate) struct SafeLogFingerprint {
    digest: [u8; DISPLAY_DIGEST_BYTES],
    byte_len: usize,
}

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
    fn fingerprint_is_stable_correlatable_and_never_echoes_input() {
        let marker = "secret-url?token=abc\r\nforged-event";
        let first = safe_log_fingerprint(marker).to_string();
        let second = safe_log_fingerprint(marker).to_string();
        let other = safe_log_fingerprint("different").to_string();

        assert_eq!(first, second);
        assert_ne!(first, other);
        assert!(first.starts_with("sha256:"));
        assert!(first.ends_with(&format!(" bytes={}", marker.len())));
        assert!(!first.contains(marker));
        assert!(!first.contains('\r'));
        assert!(!first.contains('\n'));
    }
}
