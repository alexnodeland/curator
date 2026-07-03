//! Note identity — `kp_id` (contract: `contracts/kp-note/v1.md`).
//!
//! Identity is *minted*, never derived from content or location. The
//! `checksum` frontmatter field is exclusively a change token; using it as
//! identity would silently merge distinct notes with identical bodies.

use std::fmt;
use std::str::FromStr;

/// A producer-namespaced note identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KpId {
    /// `curio:<uuidv7>` — minted by the Curio reader at save time.
    Curio(String),
    /// `zotero:<itemKey>` — a Zotero item key.
    Zotero(String),
    /// `kp:<uuidv7>` — born-in-plane notes (e.g. librarian digests).
    Kp(String),
    /// `path:<vault-relative-path>` — implicit fallback for plain vault
    /// notes without a `kp_id`. Documented as rename-fragile.
    Path(String),
}

/// Errors from parsing a `kp_id` string.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdError {
    /// The string has no `namespace:` prefix.
    #[error("kp_id has no namespace prefix: {0:?}")]
    MissingNamespace(String),
    /// The namespace is not one of `curio`, `zotero`, `kp`, `path`.
    #[error("unknown kp_id namespace: {0:?}")]
    UnknownNamespace(String),
    /// The identifier after the namespace is empty.
    #[error("empty identifier after namespace {0:?}")]
    EmptyIdentifier(String),
}

impl FromStr for KpId {
    type Err = IdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (ns, rest) = s
            .split_once(':')
            .ok_or_else(|| IdError::MissingNamespace(s.to_owned()))?;
        if rest.is_empty() {
            return Err(IdError::EmptyIdentifier(ns.to_owned()));
        }
        match ns {
            "curio" => Ok(KpId::Curio(rest.to_owned())),
            "zotero" => Ok(KpId::Zotero(rest.to_owned())),
            "kp" => Ok(KpId::Kp(rest.to_owned())),
            "path" => Ok(KpId::Path(rest.to_owned())),
            other => Err(IdError::UnknownNamespace(other.to_owned())),
        }
    }
}

impl fmt::Display for KpId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KpId::Curio(id) => write!(f, "curio:{id}"),
            KpId::Zotero(id) => write!(f, "zotero:{id}"),
            KpId::Kp(id) => write!(f, "kp:{id}"),
            KpId::Path(p) => write!(f, "path:{p}"),
        }
    }
}

impl serde::Serialize for KpId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for KpId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_namespace() {
        for raw in [
            "curio:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d",
            "zotero:AB2C3DEF",
            "kp:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6e",
            "path:notes/some-note.md",
        ] {
            let id: KpId = raw.parse().expect("should parse");
            assert_eq!(id.to_string(), raw);
        }
    }

    #[test]
    fn path_ids_keep_embedded_colons() {
        let id: KpId = "path:a:b/c.md".parse().expect("should parse");
        assert_eq!(id, KpId::Path("a:b/c.md".to_owned()));
    }

    #[test]
    fn rejects_missing_namespace() {
        let err = "no-colon-here".parse::<KpId>().unwrap_err();
        assert!(matches!(err, IdError::MissingNamespace(_)));
    }

    #[test]
    fn rejects_unknown_namespace() {
        let err = "bogus:123".parse::<KpId>().unwrap_err();
        assert_eq!(err, IdError::UnknownNamespace("bogus".to_owned()));
    }

    #[test]
    fn rejects_empty_identifier() {
        let err = "curio:".parse::<KpId>().unwrap_err();
        assert_eq!(err, IdError::EmptyIdentifier("curio".to_owned()));
    }

    #[test]
    fn serde_round_trips_as_a_string() {
        let id: KpId = "zotero:AB2C3DEF".parse().expect("parses");
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, "\"zotero:AB2C3DEF\"");
        let back: KpId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }

    /// Any non-empty identifier in any namespace — including identifiers
    /// containing further colons, slashes, unicode — must survive
    /// format -> parse unchanged. This is the identity round-trip the
    /// whole index keys on.
    mod properties {
        use super::*;
        use proptest::prelude::*;

        fn arb_kp_id() -> impl Strategy<Value = KpId> {
            let ident = proptest::string::string_regex(".{1,64}")
                .expect("valid regex")
                .prop_filter("identifier must be non-empty", |s| !s.is_empty());
            (0..4u8, ident).prop_map(|(ns, id)| match ns {
                0 => KpId::Curio(id),
                1 => KpId::Zotero(id),
                2 => KpId::Kp(id),
                _ => KpId::Path(id),
            })
        }

        proptest! {
            #[test]
            fn format_then_parse_round_trips(id in arb_kp_id()) {
                let formatted = id.to_string();
                let parsed: KpId = formatted.parse().expect("formatted ids must parse");
                prop_assert_eq!(parsed, id);
            }

            #[test]
            fn parse_then_format_is_identity_on_valid_strings(
                ns in prop_oneof![Just("curio"), Just("zotero"), Just("kp"), Just("path")],
                ident in ".{1,64}",
            ) {
                prop_assume!(!ident.is_empty());
                let raw = format!("{ns}:{ident}");
                let parsed: KpId = raw.parse().expect("valid ids must parse");
                prop_assert_eq!(parsed.to_string(), raw);
            }

            #[test]
            fn arbitrary_strings_never_panic(s in ".{0,64}") {
                let _ = s.parse::<KpId>();
            }
        }
    }
}
