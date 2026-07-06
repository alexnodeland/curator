//! The ONE query layer under the MCP surface — and under the CLI's
//! `search` / `get` / `related` / `recent` commands, so exercising either
//! entrypoint exercises the same logic.
//!
//! [`KpEngine`] holds the loaded config plus the configured embedder and
//! opens a fresh read-only [`IndexReader`] per operation (cheap by
//! design — see curator-index's writer-discipline docs; per-session stdio
//! servers and CLI one-shots get snapshot-consistent reads under WAL).
//! The only write verb, [`KpEngine::propose`], rides `proposals/v1` via
//! curator-core's `create_proposal` — there is no other write path.

use curator_core::{KpConfig, ProposalWriteError, Vault, VaultError};
use curator_index::embed::EmbedError;
use curator_index::{Embedder, IndexError, IndexReader, SearchHit, embedder_from_config};

use crate::types::{
    DigestNoteOutput, DigestOutput, FrontmatterOutput, HitOutput, IndexMetaOutput, LinkOutput,
    NoteKind, NoteOutput, ProposeFileArg, ProposeOutput, RecentNoteOutput, RecentOutput,
    RelatedOutput, SearchMode, SearchOutput,
};

/// Default result count for `kp_search` / `kp_related` (contract).
pub const DEFAULT_K: u32 = 10;
/// Default look-back window for `kp_recent`, in days (contract).
pub const DEFAULT_DAYS: u32 = 7;
/// Hard cap on `kp_recent` rows (documented in the contract's output
/// shapes) — bounded responses, whatever the window.
pub const RECENT_CAP: usize = 50;
/// The `author` stamped on proposals created through this surface.
pub const PROPOSAL_AUTHOR: &str = "curator-mcp";

/// Errors from engine operations. `Display` strings are what MCP clients
/// see as tool-error content — keep them self-explanatory.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Index(#[from] IndexError),
    #[error(transparent)]
    Embed(#[from] EmbedError),
    #[error(transparent)]
    Vault(#[from] VaultError),
    #[error(transparent)]
    Proposal(#[from] ProposalWriteError),
    /// The id is not in the index (any namespace).
    #[error("no note with id {0:?} in the index")]
    UnknownId(String),
}

/// The shared query layer: config + embedder, one reader per operation.
pub struct KpEngine {
    config: KpConfig,
    embedder: Box<dyn Embedder>,
}

impl std::fmt::Debug for KpEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KpEngine")
            .field("index", &self.config.index.path)
            .field("embedder", &self.embedder.id())
            .finish()
    }
}

impl KpEngine {
    /// Build the engine from a loaded config: resolves the embedder
    /// (`[index].embedder`) but opens nothing yet — a missing index
    /// surfaces per-operation, so `curator mcp serve` can start before the
    /// first epoch build.
    pub fn from_config(config: KpConfig) -> Result<Self, EngineError> {
        let embedder = embedder_from_config(&config)?;
        Ok(Self { config, embedder })
    }

    /// The loaded config (bind address, transport, digest dir, ...).
    #[must_use]
    pub fn config(&self) -> &KpConfig {
        &self.config
    }

    fn reader(&self) -> Result<IndexReader, EngineError> {
        Ok(IndexReader::open(self.config.index_path())?)
    }

    /// `kp_search`: ranked notes for a free-text query.
    pub fn search(
        &self,
        query: &str,
        k: Option<u32>,
        mode: Option<SearchMode>,
    ) -> Result<SearchOutput, EngineError> {
        let k = k.unwrap_or(DEFAULT_K) as usize;
        let mode = mode.unwrap_or_default();
        let reader = self.reader()?;
        let hits = match mode {
            SearchMode::Hybrid => reader.hybrid_search(self.embedder.as_ref(), query, k)?,
            SearchMode::Vector => reader.vec_search(self.embedder.as_ref(), query, k)?,
            SearchMode::Fts => reader.fts_search(query, k)?,
        };
        Ok(SearchOutput {
            mode,
            results: hits.into_iter().map(hit_output).collect(),
        })
    }

    /// `kp_get_note`: full content + frontmatter + index metadata.
    pub fn get_note(&self, id: &str) -> Result<NoteOutput, EngineError> {
        let reader = self.reader()?;
        let note = reader
            .get_note(id)?
            .ok_or_else(|| EngineError::UnknownId(id.to_owned()))?;
        let links = reader
            .links_from(id)?
            .into_iter()
            .map(|(to, kind)| LinkOutput { to, kind })
            .collect();
        Ok(NoteOutput {
            id: note.kp_id,
            title: note.title,
            path: note.path,
            content: note.body,
            frontmatter: FrontmatterOutput {
                tags: note.tags,
                source: note.source,
                created: note.created,
                updated: note.updated,
                checksum: note.checksum,
            },
            index: IndexMetaOutput {
                ingested_at: note.ingested_at,
                links,
            },
        })
    }

    /// `kp_related`: embedding-nearest notes to an existing note.
    pub fn related(&self, id: &str, k: Option<u32>) -> Result<RelatedOutput, EngineError> {
        let k = k.unwrap_or(DEFAULT_K) as usize;
        let reader = self.reader()?;
        if reader.get_note(id)?.is_none() {
            return Err(EngineError::UnknownId(id.to_owned()));
        }
        let hits = reader.related(id, k)?;
        Ok(RelatedOutput {
            id: id.to_owned(),
            results: hits.into_iter().map(hit_output).collect(),
        })
    }

    /// `kp_recent`: notes the index wrote in the last `days` days.
    pub fn recent(
        &self,
        days: Option<u32>,
        kind: Option<NoteKind>,
    ) -> Result<RecentOutput, EngineError> {
        let days = days.unwrap_or(DEFAULT_DAYS);
        let cutoff_secs = curator_core::time::unix_now().saturating_sub(u64::from(days) * 86_400);
        let cutoff = curator_core::time::rfc3339_utc(cutoff_secs);
        let reader = self.reader()?;
        let notes = reader
            .recent_since(&cutoff, kind.map(NoteKind::namespace), RECENT_CAP)?
            .into_iter()
            .map(|n| RecentNoteOutput {
                id: n.kp_id,
                title: n.title,
                path: n.path,
                tags: n.tags,
                source: n.source,
                updated: n.updated,
                ingested_at: n.ingested_at,
            })
            .collect();
        Ok(RecentOutput { days, kind, notes })
    }

    /// `kp_propose`: the ONLY write verb — creates a `proposals/v1`
    /// changeset in the vault and returns its id. Target files are never
    /// written; `curator apply` does that after human review.
    pub fn propose(
        &self,
        title: &str,
        rationale: &str,
        files: &[ProposeFileArg],
    ) -> Result<ProposeOutput, EngineError> {
        let vault = Vault::open(self.config.vault_path())?;
        let files: Vec<curator_core::ProposalFile> = files
            .iter()
            .map(|f| curator_core::ProposalFile {
                path: f.path.clone(),
                content: f.content.clone(),
            })
            .collect();
        let proposals_dir = &self.config.vault.proposals_dir;
        let proposal = curator_core::create_proposal(
            &vault,
            proposals_dir,
            PROPOSAL_AUTHOR,
            title,
            rationale,
            &files,
        )?;
        Ok(ProposeOutput {
            dir: format!("{}/{}", proposals_dir.trim_end_matches('/'), proposal.id),
            id: proposal.id,
            status: "open".to_owned(),
            files: proposal.files,
        })
    }

    /// `kp_digest_latest`: the newest librarian digest note, if any.
    pub fn digest_latest(&self) -> Result<DigestOutput, EngineError> {
        let reader = self.reader()?;
        let digest = reader
            .latest_digest(&self.config.librarian.digest_dir)?
            .map(|n| DigestNoteOutput {
                id: n.kp_id,
                title: n.title,
                path: n.path,
                content: n.body,
                created: n.created,
                ingested_at: n.ingested_at,
            });
        Ok(DigestOutput { digest })
    }
}

fn hit_output(hit: SearchHit) -> HitOutput {
    HitOutput {
        id: hit.kp_id,
        title: hit.title,
        path: hit.path,
        snippet: hit.snippet,
        score: hit.score,
    }
}
