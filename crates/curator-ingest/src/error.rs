//! curator-ingest error types.

use std::path::PathBuf;

/// Errors from ingest operations. Note-level trouble (parse failures,
/// schema violations, malformed event lines) is deliberately NOT here —
/// those are warnings + skips by contract; only environment-level failure
/// (vault, index, I/O, embedding backend) aborts a run.
#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    /// Vault access failed (bad root, unreadable file, path escape).
    #[error(transparent)]
    Vault(#[from] curator_core::VaultError),
    /// The index refused (missing, schema/embedder mismatch, SQLite).
    #[error(transparent)]
    Index(#[from] curator_index::IndexError),
    /// The embedding backend failed.
    #[error(transparent)]
    Embed(#[from] curator_index::embed::EmbedError),
    /// Filesystem trouble outside the vault (events dir, index parent).
    #[error("ingest I/O on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
