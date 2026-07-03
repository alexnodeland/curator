//! `kp-note/v1` enrichment frontmatter (contract: `contracts/kp-note/v1.md`).
//!
//! Notes MAY carry this block; producer-generated notes always do. There is
//! deliberately **no `status` field** — lifecycle lives index-side, never in
//! note frontmatter, because producer re-exports re-render whole files and
//! would silently clobber injected fields.

use serde::{Deserialize, Serialize};

/// The `kp_schema` value this crate implements.
pub const KP_NOTE_SCHEMA: &str = "kp-note/v1";

/// The `kp-note/v1` frontmatter block.
///
/// `checksum` is a change token ONLY, never identity (see `KpId`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoteFrontmatter {
    /// Producer-namespaced identity, e.g. `curio:<uuidv7>`.
    pub kp_id: String,
    /// Always `kp-note/v1` for this version.
    pub kp_schema: String,
    /// `sha256:<hex>` change token over note content. Never identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_contract_example() {
        // Mirrors the YAML example in contracts/kp-note/v1.md (as JSON —
        // structurally identical for this flat block).
        let raw = r#"{
            "kp_id": "curio:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d",
            "kp_schema": "kp-note/v1",
            "checksum": "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
            "title": "Article title",
            "created": "2026-07-03T09:15:00Z",
            "updated": "2026-07-03T10:00:00Z",
            "tags": ["rust", "databases"],
            "source": "https://example.com/article"
        }"#;
        let fm: NoteFrontmatter = serde_json::from_str(raw).expect("should parse");
        assert_eq!(fm.kp_schema, KP_NOTE_SCHEMA);
        assert_eq!(fm.tags, vec!["rust", "databases"]);
        assert_eq!(fm.source.as_deref(), Some("https://example.com/article"));
    }

    #[test]
    fn optional_fields_default() {
        let raw = r#"{"kp_id": "kp:x", "kp_schema": "kp-note/v1", "title": "t"}"#;
        let fm: NoteFrontmatter = serde_json::from_str(raw).expect("should parse");
        assert!(fm.checksum.is_none());
        assert!(fm.tags.is_empty());
        assert!(fm.source.is_none());
    }
}
