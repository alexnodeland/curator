//! The MCP surface v1 argument and output types
//! (contract: `contracts/mcp/v1.md` — names and shapes ARE the contract).
//!
//! Every type here derives `JsonSchema`, so the rmcp router advertises
//! the exact input/output schemas the contract documents. Outputs also
//! derive `Deserialize` so the CLI, tests, and downstream tooling can
//! consume them as data rather than text.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `kp_search` retrieval mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// Reciprocal-rank fusion over the FTS and vector legs (default).
    #[default]
    Hybrid,
    /// Embedding KNN only.
    Vector,
    /// BM25 full-text only.
    Fts,
}

impl std::fmt::Display for SearchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SearchMode::Hybrid => "hybrid",
            SearchMode::Vector => "vector",
            SearchMode::Fts => "fts",
        })
    }
}

/// A `kp-note/v1` identity namespace — the `kind` filter of `kp_recent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum NoteKind {
    Curio,
    Zotero,
    Kp,
    Path,
}

impl NoteKind {
    /// The namespace prefix (without the colon).
    #[must_use]
    pub fn namespace(self) -> &'static str {
        match self {
            NoteKind::Curio => "curio",
            NoteKind::Zotero => "zotero",
            NoteKind::Kp => "kp",
            NoteKind::Path => "path",
        }
    }
}

/// `kp_search` arguments.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// Free-text query.
    pub query: String,
    /// Result count (default 10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k: Option<u32>,
    /// Retrieval mode (default "hybrid").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<SearchMode>,
}

/// `kp_get_note` arguments.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GetNoteArgs {
    /// Note identity in any namespace: `curio:` | `zotero:` | `kp:` | `path:`.
    pub id: String,
}

/// `kp_related` arguments.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RelatedArgs {
    /// Note identity in any namespace.
    pub id: String,
    /// Result count (default 10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k: Option<u32>,
}

/// `kp_recent` arguments.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RecentArgs {
    /// Look-back window in days (default 7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days: Option<u32>,
    /// Identity-namespace filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<NoteKind>,
}

/// One proposed file in `kp_propose`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProposeFileArg {
    /// Vault-relative path, forward slashes.
    pub path: String,
    /// The complete intended content of the file.
    pub content: String,
}

/// `kp_propose` arguments.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProposeArgs {
    pub title: String,
    pub rationale: String,
    /// The full new content of every file the proposal touches.
    pub files: Vec<ProposeFileArg>,
}

/// One ranked hit (`kp_search`, `kp_related`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HitOutput {
    /// `kp_id` in its identity namespace.
    pub id: String,
    pub title: String,
    /// Vault-relative path.
    pub path: String,
    /// Match context (FTS highlight or best-chunk excerpt).
    pub snippet: String,
    /// Higher is better; comparable only within one response.
    pub score: f64,
}

/// `kp_search` output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SearchOutput {
    /// The mode that served the query.
    pub mode: SearchMode,
    /// Ranked hits, best first.
    pub results: Vec<HitOutput>,
}

/// The frontmatter block of `kp_get_note` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FrontmatterOutput {
    pub tags: Vec<String>,
    pub source: Option<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
    /// Change token (`sha256:<hex>`), never identity.
    pub checksum: Option<String>,
}

/// One outgoing edge in `kp_get_note` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LinkOutput {
    /// Target `kp_id`.
    pub to: String,
    /// Edge kind (e.g. `wikilink`, `markdown`).
    pub kind: String,
}

/// The index-metadata block of `kp_get_note` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IndexMetaOutput {
    /// When the index last wrote this note (RFC 3339 UTC).
    pub ingested_at: String,
    /// Outgoing edges recorded at ingest.
    pub links: Vec<LinkOutput>,
}

/// `kp_get_note` output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NoteOutput {
    pub id: String,
    pub title: String,
    /// Vault-relative path.
    pub path: String,
    /// Full markdown body as indexed.
    pub content: String,
    pub frontmatter: FrontmatterOutput,
    pub index: IndexMetaOutput,
}

/// `kp_related` output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RelatedOutput {
    /// The anchor note's id, echoed.
    pub id: String,
    /// Embedding-nearest notes, best first (the anchor itself excluded).
    pub results: Vec<HitOutput>,
}

/// One row of `kp_recent` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecentNoteOutput {
    pub id: String,
    pub title: String,
    pub path: String,
    pub tags: Vec<String>,
    pub source: Option<String>,
    /// Frontmatter-declared update timestamp, when present.
    pub updated: Option<String>,
    /// When the index last wrote this note (RFC 3339 UTC).
    pub ingested_at: String,
}

/// `kp_recent` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecentOutput {
    /// The effective look-back window.
    pub days: u32,
    /// The effective namespace filter, when one was given.
    pub kind: Option<NoteKind>,
    /// Newest first (by index write time), capped at 50.
    pub notes: Vec<RecentNoteOutput>,
}

/// `kp_propose` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProposeOutput {
    /// The proposal ULID (also its directory name).
    pub id: String,
    /// Always `open` on creation.
    pub status: String,
    /// Vault-relative proposal directory (`<proposals_dir>/<id>`).
    pub dir: String,
    /// Vault-relative paths the proposal touches.
    pub files: Vec<String>,
}

/// The digest note inside `kp_digest_latest` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DigestNoteOutput {
    pub id: String,
    pub title: String,
    pub path: String,
    /// Full markdown body as indexed.
    pub content: String,
    pub created: Option<String>,
    pub ingested_at: String,
}

/// `kp_digest_latest` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DigestOutput {
    /// `null` when no digest exists yet.
    pub digest: Option<DigestNoteOutput>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_mode_serializes_to_the_contract_strings() {
        for (mode, s) in [
            (SearchMode::Hybrid, "\"hybrid\""),
            (SearchMode::Vector, "\"vector\""),
            (SearchMode::Fts, "\"fts\""),
        ] {
            assert_eq!(serde_json::to_string(&mode).expect("serialize"), s);
            let back: SearchMode = serde_json::from_str(s).expect("parse");
            assert_eq!(back, mode);
        }
        assert!(serde_json::from_str::<SearchMode>("\"HYBRID\"").is_err());
    }

    #[test]
    fn args_accept_the_contract_shapes() {
        let args: SearchArgs =
            serde_json::from_str(r#"{"query":"rust"}"#).expect("k and mode optional");
        assert_eq!(args.k, None);
        assert_eq!(args.mode, None);
        let args: SearchArgs =
            serde_json::from_str(r#"{"query":"rust","k":3,"mode":"fts"}"#).expect("full form");
        assert_eq!(args.k, Some(3));
        assert_eq!(args.mode, Some(SearchMode::Fts));

        let args: RecentArgs = serde_json::from_str(r#"{}"#).expect("all optional");
        assert_eq!(args.days, None);
        let args: RecentArgs =
            serde_json::from_str(r#"{"days":30,"kind":"zotero"}"#).expect("full form");
        assert_eq!(args.kind, Some(NoteKind::Zotero));
        assert_eq!(args.kind.expect("set").namespace(), "zotero");
    }
}
