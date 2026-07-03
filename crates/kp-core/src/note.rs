//! `kp-note/v1` enrichment frontmatter + the note model
//! (contract: `contracts/kp-note/v1.md`).
//!
//! Notes MAY carry the KP block; producer-generated notes always do. There
//! is deliberately **no `status` field** — lifecycle lives index-side,
//! never in note frontmatter, because producer re-exports re-render whole
//! files and would silently clobber injected fields. Unknown frontmatter
//! keys are PRESERVED on round-trip (contract binding rule 3): everything
//! outside the KP block belongs to the user or other tools.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::checksum::Checksum;
use crate::id::KpId;

/// The `kp_schema` value this crate implements.
pub const KP_NOTE_SCHEMA: &str = "kp-note/v1";

/// Errors from parsing a note.
#[derive(Debug, thiserror::Error)]
pub enum NoteError {
    /// The frontmatter block is not valid YAML (or not a YAML mapping).
    #[error("invalid YAML frontmatter in {path}: {source}")]
    Yaml {
        /// Vault-relative path of the offending note.
        path: String,
        /// The underlying YAML error.
        #[source]
        source: serde_yaml::Error,
    },
    /// The frontmatter carries `kp_id` but the KP block does not conform
    /// to kp-note/v1 (bad id namespace, bad checksum, missing title, ...).
    #[error("frontmatter in {path} is not valid kp-note/v1: {source}")]
    Contract {
        /// Vault-relative path of the offending note.
        path: String,
        /// The underlying deserialization error.
        #[source]
        source: serde_yaml::Error,
    },
    /// Frontmatter opened with `---` but never closed.
    #[error("unterminated frontmatter block in {path}")]
    Unterminated {
        /// Vault-relative path of the offending note.
        path: String,
    },
}

/// The `kp-note/v1` frontmatter block.
///
/// `checksum` is a change token ONLY, never identity (see [`Checksum`]).
/// Unknown keys land in `extra` and are written back verbatim on
/// serialization — the plane never eats another tool's frontmatter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoteFrontmatter {
    /// Producer-namespaced identity, e.g. `curio:<uuidv7>`.
    pub kp_id: KpId,
    /// Always `kp-note/v1` for this version.
    pub kp_schema: String,
    /// `sha256:<hex>` change token over the note body. Never identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<Checksum>,
    /// Human title.
    pub title: String,
    /// RFC 3339 UTC creation timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    /// RFC 3339 UTC last-update timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
    /// Free-form tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Origin URL; `None` for born-in-vault notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Every frontmatter key that is not part of the KP block, preserved
    /// as-is (contract binding rule 3).
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

impl NoteFrontmatter {
    /// A minimal conforming block.
    #[must_use]
    pub fn new(kp_id: KpId, title: impl Into<String>) -> Self {
        Self {
            kp_id,
            kp_schema: KP_NOTE_SCHEMA.to_owned(),
            checksum: None,
            title: title.into(),
            created: None,
            updated: None,
            tags: Vec::new(),
            source: None,
            extra: BTreeMap::new(),
        }
    }
}

/// What sits between the `---` fences of a note, as the plane sees it.
#[derive(Debug, Clone, PartialEq)]
pub enum Frontmatter {
    /// No frontmatter block at all.
    None,
    /// A YAML block that carries no `kp_id` — some other tool's metadata.
    /// Kept verbatim (byte-exact) so round-tripping never rewrites it.
    Foreign(String),
    /// A conforming kp-note/v1 block (unknown keys preserved in `extra`).
    Kp(NoteFrontmatter),
}

/// A note: vault-relative path + frontmatter + markdown body.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    /// Vault-relative path, forward slashes (e.g. `curio/some-article.md`).
    pub rel_path: String,
    /// The parsed frontmatter block (or [`Frontmatter::None`]).
    pub frontmatter: Frontmatter,
    /// The markdown body (everything after the closing `---`, or the whole
    /// file when there is no frontmatter).
    pub body: String,
}

impl Note {
    /// Parse a note from its full file content.
    pub fn parse(rel_path: impl Into<String>, content: &str) -> Result<Self, NoteError> {
        let rel_path = rel_path.into();
        let Some(split) = split_frontmatter(content) else {
            return Ok(Self {
                rel_path,
                frontmatter: Frontmatter::None,
                body: content.to_owned(),
            });
        };
        let (yaml, body) = split.map_err(|()| NoteError::Unterminated {
            path: rel_path.clone(),
        })?;
        // Peek for `kp_id` on the loose mapping first: a block without it
        // is foreign metadata we must neither validate nor rewrite.
        let loose: serde_yaml::Value =
            serde_yaml::from_str(yaml).map_err(|source| NoteError::Yaml {
                path: rel_path.clone(),
                source,
            })?;
        let has_kp_id = loose.get("kp_id").is_some();
        let frontmatter = if has_kp_id {
            let fm: NoteFrontmatter =
                serde_yaml::from_str(yaml).map_err(|source| NoteError::Contract {
                    path: rel_path.clone(),
                    source,
                })?;
            Frontmatter::Kp(fm)
        } else {
            Frontmatter::Foreign(yaml.to_owned())
        };
        Ok(Self {
            rel_path,
            frontmatter,
            body: body.to_owned(),
        })
    }

    /// Serialize back to full file content.
    ///
    /// `Foreign` blocks are emitted byte-exact; `Kp` blocks re-render
    /// through serde (semantically, not byte, stable — unknown keys ride
    /// along in `extra`).
    #[must_use]
    pub fn to_markdown(&self) -> String {
        match &self.frontmatter {
            Frontmatter::None => self.body.clone(),
            Frontmatter::Foreign(yaml) => format!("---\n{yaml}---\n{}", self.body),
            Frontmatter::Kp(fm) => {
                let yaml = serde_yaml::to_string(fm).expect("kp-note/v1 block always serializes");
                format!("---\n{yaml}---\n{}", self.body)
            }
        }
    }

    /// The note's identity: the frontmatter `kp_id` when present, else the
    /// `path:` fallback (documented rename-fragile, per contract).
    #[must_use]
    pub fn kp_id(&self) -> KpId {
        match &self.frontmatter {
            Frontmatter::Kp(fm) => fm.kp_id.clone(),
            Frontmatter::None | Frontmatter::Foreign(_) => KpId::Path(self.rel_path.clone()),
        }
    }

    /// The note's title: frontmatter `title` when present, else the file
    /// stem of the vault-relative path.
    #[must_use]
    pub fn title(&self) -> String {
        match &self.frontmatter {
            Frontmatter::Kp(fm) => fm.title.clone(),
            Frontmatter::None | Frontmatter::Foreign(_) => {
                let name = self.rel_path.rsplit('/').next().unwrap_or(&self.rel_path);
                name.strip_suffix(".md").unwrap_or(name).to_owned()
            }
        }
    }

    /// The change token over the note body.
    #[must_use]
    pub fn body_checksum(&self) -> Checksum {
        Checksum::compute(self.body.as_bytes())
    }
}

/// Split `content` into `(yaml, body)` when it opens with a `---`
/// frontmatter fence.
///
/// - `None`: no frontmatter block (content does not start with `---`).
/// - `Some(Err(()))`: opened but never closed.
/// - `Some(Ok((yaml, body)))`: `yaml` is everything between the fences
///   (trailing newline included), `body` is everything after the closing
///   fence line.
#[allow(clippy::result_unit_err)] // the only failure is "unterminated"; callers wrap it with path context
pub fn split_frontmatter(content: &str) -> Option<Result<(&str, &str), ()>> {
    let rest = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))?;
    // Find the closing fence: a line that is exactly `---` (allowing \r).
    let mut offset = 0;
    // split_inclusive also yields a final segment with no trailing newline,
    // so a bare closing `---` at EOF is handled inside the loop.
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            let yaml = &rest[..offset];
            let body = &rest[offset + line.len()..];
            return Some(Ok((yaml, body)));
        }
        offset += line.len();
    }
    Some(Err(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact YAML example from contracts/kp-note/v1.md.
    const CONTRACT_EXAMPLE: &str = r#"---
kp_id: "curio:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d"
kp_schema: kp-note/v1
checksum: "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
title: "Article title"
created: 2026-07-03T09:15:00Z
updated: 2026-07-03T10:00:00Z
tags: [rust, databases]
source: "https://example.com/article"
---

Body text here.
"#;

    #[test]
    fn parses_the_contract_example() {
        let note = Note::parse("curio/article.md", CONTRACT_EXAMPLE).expect("parses");
        let Frontmatter::Kp(fm) = &note.frontmatter else {
            panic!("expected a KP block");
        };
        assert_eq!(
            fm.kp_id,
            KpId::Curio("0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d".to_owned())
        );
        assert_eq!(fm.kp_schema, KP_NOTE_SCHEMA);
        assert_eq!(fm.title, "Article title");
        assert_eq!(fm.tags, vec!["rust", "databases"]);
        assert_eq!(fm.source.as_deref(), Some("https://example.com/article"));
        assert_eq!(fm.created.as_deref(), Some("2026-07-03T09:15:00Z"));
        assert!(fm.extra.is_empty());
        assert_eq!(note.body, "\nBody text here.\n");
        assert_eq!(
            note.kp_id().to_string(),
            "curio:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d"
        );
    }

    #[test]
    fn kp_frontmatter_round_trips_semantically() {
        let note = Note::parse("n.md", CONTRACT_EXAMPLE).expect("parses");
        let rendered = note.to_markdown();
        let back = Note::parse("n.md", &rendered).expect("re-parses");
        assert_eq!(note.frontmatter, back.frontmatter);
        assert_eq!(note.body, back.body);
    }

    #[test]
    fn unknown_frontmatter_keys_are_preserved() {
        // Contract binding rule 3: everything outside the KP block belongs
        // to the user or other tools — it must survive a round-trip.
        let raw = "---\nkp_id: \"kp:abc\"\nkp_schema: kp-note/v1\ntitle: T\naliases: [x, y]\ncustom_weight: 3\n---\nbody\n";
        let note = Note::parse("n.md", raw).expect("parses");
        let Frontmatter::Kp(fm) = &note.frontmatter else {
            panic!("expected KP block");
        };
        assert_eq!(fm.extra.len(), 2);
        assert!(fm.extra.contains_key("aliases"));
        let rendered = note.to_markdown();
        assert!(
            rendered.contains("aliases"),
            "unknown key eaten: {rendered}"
        );
        assert!(
            rendered.contains("custom_weight"),
            "unknown key eaten: {rendered}"
        );
        let back = Note::parse("n.md", &rendered).expect("re-parses");
        assert_eq!(note.frontmatter, back.frontmatter);
    }

    #[test]
    fn note_without_frontmatter_falls_back_to_path_identity() {
        let note = Note::parse("ideas/thing.md", "# Just markdown\n").expect("parses");
        assert_eq!(note.frontmatter, Frontmatter::None);
        assert_eq!(note.body, "# Just markdown\n");
        assert_eq!(note.kp_id(), KpId::Path("ideas/thing.md".to_owned()));
        assert_eq!(note.title(), "thing");
        assert_eq!(note.to_markdown(), "# Just markdown\n");
    }

    #[test]
    fn foreign_frontmatter_is_kept_byte_exact() {
        // A block with no kp_id is another tool's metadata: never validated,
        // never re-rendered.
        let raw = "---\nweird:   [1, 2,   3]\n# a comment other tools care about\n---\nbody\n";
        let note = Note::parse("n.md", raw).expect("parses");
        assert!(matches!(note.frontmatter, Frontmatter::Foreign(_)));
        assert_eq!(note.kp_id(), KpId::Path("n.md".to_owned()));
        assert_eq!(
            note.to_markdown(),
            raw,
            "foreign frontmatter must round-trip byte-exact"
        );
    }

    #[test]
    fn bad_kp_block_is_a_contract_error() {
        // Has kp_id but a bogus namespace — must fail loudly, not fall back.
        let raw = "---\nkp_id: \"bogus:1\"\nkp_schema: kp-note/v1\ntitle: T\n---\nbody\n";
        let err = Note::parse("n.md", raw).unwrap_err();
        assert!(matches!(err, NoteError::Contract { .. }));
        // Bad checksum shape too.
        let raw = "---\nkp_id: \"kp:1\"\nkp_schema: kp-note/v1\nchecksum: nope\ntitle: T\n---\n";
        assert!(matches!(
            Note::parse("n.md", raw).unwrap_err(),
            NoteError::Contract { .. }
        ));
    }

    #[test]
    fn unterminated_frontmatter_is_an_error() {
        let raw = "---\nkp_id: \"kp:1\"\nno closing fence\n";
        assert!(matches!(
            Note::parse("n.md", raw).unwrap_err(),
            NoteError::Unterminated { .. }
        ));
    }

    #[test]
    fn split_frontmatter_edges() {
        assert!(split_frontmatter("no fences").is_none());
        assert!(split_frontmatter("").is_none());
        // `---` mid-document is not frontmatter.
        assert!(split_frontmatter("text\n---\nmore\n---\n").is_none());
        // Closing fence with no trailing newline.
        let (yaml, body) = split_frontmatter("---\na: 1\n---")
            .expect("some")
            .expect("ok");
        assert_eq!(yaml, "a: 1\n");
        assert_eq!(body, "");
        // CRLF fences.
        let (yaml, body) = split_frontmatter("---\r\na: 1\r\n---\r\nB")
            .expect("some")
            .expect("ok");
        assert_eq!(yaml, "a: 1\r\n");
        assert_eq!(body, "B");
        // A horizontal rule `----` does not close the block.
        assert!(
            split_frontmatter("---\na: 1\n----\n")
                .expect("some")
                .is_err()
        );
    }

    #[test]
    fn body_checksum_is_over_the_body_only() {
        let note = Note::parse(
            "n.md",
            "---\nkp_id: \"kp:1\"\nkp_schema: kp-note/v1\ntitle: T\n---\nsame body\n",
        )
        .expect("parses");
        let bare = Note::parse("m.md", "same body\n").expect("parses");
        // Same body, different frontmatter/identity: same CHANGE TOKEN —
        // and that is exactly why checksum must never be identity.
        assert_eq!(note.body_checksum(), bare.body_checksum());
        assert_ne!(note.kp_id(), bare.kp_id());
    }
}
