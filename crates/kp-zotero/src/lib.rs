//! Knowledge Plane Zotero producer (read-only at v1).
//!
//! Two channels:
//! 1. Zotero Web API for metadata — delta polling via
//!    `Last-Modified-Version`, `/deleted` for tombstones (deletions raise
//!    proposals, never auto-delete).
//! 2. The official `/fulltext` endpoint as the primary fulltext source,
//!    with a small CRC-verified WebDAV `.prop`/`.zip` fallback for
//!    self-hosted attachment stores.
//!
//! Literature-note stubs land in the vault via `proposals/v1`, keyed
//! strictly on `zotero:<itemKey>` — a citekey rename is a rename proposal,
//! never a duplicate stub. All tests are fixture-driven (hermetic).

/// The Zotero sync client (stub — wiring lands with the Zotero milestone).
#[derive(Debug, Default)]
pub struct ZoteroClient {
    _private: (),
}

impl ZoteroClient {
    /// Create the client.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}
