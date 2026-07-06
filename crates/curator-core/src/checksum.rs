//! The `checksum` change token (contract: `contracts/kp-note/v1.md`).
//!
//! **A change token ONLY, never identity.** Two notes with identical bodies
//! are still two notes — keying anything on checksum silently merges them
//! (see `docs/design/decisions.md` §2). The newtype exists so the type
//! system carries that rule: there is deliberately no way to obtain a
//! [`crate::KpId`] from a [`Checksum`].

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A `sha256:<64 lowercase hex>` change token.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Checksum(String);

/// Errors from parsing a checksum string.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ChecksumError {
    /// The token does not start with `sha256:`.
    #[error("checksum must start with \"sha256:\": {0:?}")]
    BadPrefix(String),
    /// The digest is not exactly 64 hex characters.
    #[error("checksum digest must be 64 hex chars, got {0} in {1:?}")]
    BadDigest(usize, String),
}

impl Checksum {
    /// Compute the checksum of raw bytes.
    #[must_use]
    pub fn compute(bytes: impl AsRef<[u8]>) -> Self {
        let digest = Sha256::digest(bytes.as_ref());
        let mut hex = String::with_capacity(64);
        for b in digest {
            use fmt::Write as _;
            let _ = write!(hex, "{b:02x}");
        }
        Self(hex)
    }

    /// The bare 64-char lowercase hex digest (no prefix).
    #[must_use]
    pub fn hex(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Checksum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sha256:{}", self.0)
    }
}

impl FromStr for Checksum {
    type Err = ChecksumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s
            .strip_prefix("sha256:")
            .ok_or_else(|| ChecksumError::BadPrefix(s.to_owned()))?;
        if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ChecksumError::BadDigest(hex.len(), s.to_owned()));
        }
        // Normalize: the canonical form is lowercase.
        Ok(Self(hex.to_ascii_lowercase()))
    }
}

impl Serialize for Checksum {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Checksum {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // sha256("") — the canonical known-answer vector.
    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn computes_known_vectors() {
        assert_eq!(Checksum::compute(b"").hex(), EMPTY);
        assert_eq!(
            Checksum::compute(b"hello world").to_string(),
            "sha256:b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn round_trips_parse_and_display() {
        let c = Checksum::compute(b"note body");
        let back: Checksum = c.to_string().parse().expect("round trip");
        assert_eq!(c, back);
    }

    #[test]
    fn uppercase_hex_normalizes() {
        let raw = format!("sha256:{}", EMPTY.to_ascii_uppercase());
        let c: Checksum = raw.parse().expect("uppercase accepted");
        assert_eq!(c.hex(), EMPTY);
    }

    #[test]
    fn rejects_missing_prefix() {
        let err = EMPTY.parse::<Checksum>().unwrap_err();
        assert!(matches!(err, ChecksumError::BadPrefix(_)));
        assert!(matches!(
            "md5:abc".parse::<Checksum>().unwrap_err(),
            ChecksumError::BadPrefix(_)
        ));
    }

    #[test]
    fn rejects_bad_digests() {
        assert!(matches!(
            "sha256:deadbeef".parse::<Checksum>().unwrap_err(),
            ChecksumError::BadDigest(8, _)
        ));
        let nonhex = format!("sha256:{}", "z".repeat(64));
        assert!(matches!(
            nonhex.parse::<Checksum>().unwrap_err(),
            ChecksumError::BadDigest(64, _)
        ));
    }

    #[test]
    fn serde_round_trips_as_a_string() {
        let c = Checksum::compute(b"x");
        let json = serde_json::to_string(&c).expect("serialize");
        assert_eq!(json, format!("\"{c}\""));
        let back: Checksum = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(c, back);
    }

    #[test]
    fn same_bytes_same_token_different_bytes_different_token() {
        assert_eq!(Checksum::compute(b"a"), Checksum::compute(b"a"));
        assert_ne!(Checksum::compute(b"a"), Checksum::compute(b"b"));
    }
}
