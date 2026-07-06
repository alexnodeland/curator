//! curator-index error types.

use std::path::PathBuf;

use crate::embed::EmbedError;

/// Errors from index operations.
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    /// An underlying SQLite error.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    /// Filesystem trouble around the db file (epoch swap, parent dirs...).
    #[error("index I/O on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// No index exists at the configured path yet.
    #[error("no index at {0} — run an epoch build first")]
    Missing(PathBuf),
    /// Another process holds the writer lock (a concurrent `curator ingest` /
    /// `curator reindex` / `curator zotero sync`). Writers are strictly single-file
    /// single-process; readers are never blocked.
    #[error(
        "another curator process is writing this index (lock held on {0}) — \
         wait for it to finish and retry"
    )]
    WriterLocked(PathBuf),
    /// The index was built by a different embedder. Mixed-model indexes
    /// are forbidden: every vector in an epoch comes from ONE model.
    #[error(
        "index was built with embedder {index_id:?} ({index_dims} dims) but \
         {embedder_id:?} ({embedder_dims} dims) was supplied — rebuild the \
         index with a new epoch (mixed-model indexes are forbidden)"
    )]
    EmbedderMismatch {
        index_id: String,
        index_dims: usize,
        embedder_id: String,
        embedder_dims: usize,
    },
    /// The on-disk schema version is not the one this binary implements.
    /// Schema changes NEVER migrate in place — they are a new epoch.
    #[error(
        "index schema version {found} != supported {supported} — rebuild \
         the index with a new epoch (schema changes are never migrated in place)"
    )]
    SchemaVersion { found: i64, supported: i64 },
    /// The meta row is missing or malformed — not a KP index (or corrupt).
    #[error("index at {0} has no readable meta row — not a curator index, or corrupt")]
    CorruptMeta(PathBuf),
    /// `PRAGMA integrity_check` (or the completeness check) failed on a
    /// freshly built epoch — the swap is refused, the serving epoch stays.
    #[error("epoch build failed verification: {0}")]
    EpochVerification(String),
    /// The embedding backend failed.
    #[error(transparent)]
    Embed(#[from] EmbedError),
    /// An embedder returned a vector of the wrong dimensionality.
    #[error("embedder {id:?} returned a {got}-dim vector, expected {expected}")]
    WrongDims {
        id: String,
        got: usize,
        expected: usize,
    },
    /// A pre-embedded upsert supplied mismatched chunk/vector counts.
    #[error("pre-embedded upsert got {chunks} chunks but {vectors} vectors")]
    ChunkVectorMismatch { chunks: usize, vectors: usize },
    /// Vault trouble while sourcing notes for an epoch build.
    #[error(transparent)]
    Vault(#[from] curator_core::VaultError),
}
