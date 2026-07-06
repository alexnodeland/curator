//! Deterministic UUIDv7 minting for born-in-plane notes (`kp:` namespace).
//!
//! A digest run must be byte-identical for identical inputs (the clock is
//! injected), so the "random" bits of the v7 UUID are derived from a
//! caller-supplied seed via SHA-256 instead of an RNG: same clock + same
//! content → same id, forever. The result is still a structurally valid
//! UUIDv7 — 48-bit unix-millisecond timestamp, version nibble `7`,
//! RFC 9562 variant bits — so it sorts by creation time like any other.

use sha2::{Digest, Sha256};

/// Mint a deterministic UUIDv7 from a unix-millisecond timestamp and a
/// seed. The seed replaces the spec's random bits (rand_a / rand_b);
/// callers wanting uniqueness must vary the seed (the digest engine seeds
/// with the digest date + rendered body).
#[must_use]
pub fn mint_uuid7(unix_ms: u64, seed: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(unix_ms.to_be_bytes());
    hasher.update(seed);
    let digest = hasher.finalize();

    let mut bytes = [0u8; 16];
    // 48-bit big-endian millisecond timestamp.
    bytes[..6].copy_from_slice(&unix_ms.to_be_bytes()[2..8]);
    // Everything else from the seed hash…
    bytes[6..].copy_from_slice(&digest[..10]);
    // …with version 7 and the RFC 9562 variant stamped over it.
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let h = |range: std::ops::Range<usize>| {
        bytes[range].iter().fold(String::new(), |mut acc, b| {
            acc.push_str(&format!("{b:02x}"));
            acc
        })
    };
    format!(
        "{}-{}-{}-{}-{}",
        h(0..4),
        h(4..6),
        h(6..8),
        h(8..10),
        h(10..16)
    )
}

/// Is this string shaped like a (lowercase-hex) UUIDv7? Checks the
/// canonical 8-4-4-4-12 layout, the version nibble `7` and the RFC 9562
/// variant — what the auto-apply gate requires of a digest note's `kp:`
/// identifier.
#[must_use]
pub fn is_uuid7(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (i, b) in bytes.iter().enumerate() {
        match i {
            8 | 13 | 18 | 23 => {
                if *b != b'-' {
                    return false;
                }
            }
            _ => {
                if !b.is_ascii_hexdigit() || b.is_ascii_uppercase() {
                    return false;
                }
            }
        }
    }
    bytes[14] == b'7' && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minting_is_deterministic() {
        let a = mint_uuid7(1_783_070_100_000, b"seed");
        let b = mint_uuid7(1_783_070_100_000, b"seed");
        assert_eq!(a, b);
        assert_ne!(a, mint_uuid7(1_783_070_100_000, b"other seed"));
        assert_ne!(a, mint_uuid7(1_783_070_100_001, b"seed"));
    }

    #[test]
    fn minted_ids_are_valid_uuid7() {
        let id = mint_uuid7(1_783_070_100_000, b"seed");
        assert!(is_uuid7(&id), "{id}");
        // Timestamp-ordered: a later clock sorts later.
        let later = mint_uuid7(1_783_070_200_000, b"seed");
        assert!(id < later);
    }

    #[test]
    fn shape_check_rejects_non_v7() {
        assert!(is_uuid7("0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d"));
        for bad in [
            "",
            "not-a-uuid",
            "0197b2c4-8f3e-4cc1-a5d2-3e9f10aa4b6d", // v4
            "0197b2c4-8f3e-7cc1-c5d2-3e9f10aa4b6d", // bad variant
            "0197B2C4-8F3E-7CC1-A5D2-3E9F10AA4B6D", // uppercase
            "0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6",  // short
        ] {
            assert!(!is_uuid7(bad), "{bad:?} must be rejected");
        }
    }
}
