//! Knowledge Plane ingest — producers → vault/index.
//!
//! Producers integrate by ADAPTER, never by template: this crate consumes
//! a producer's own published formats and maps them onto `kp-note/v1`
//! identities. Anything that writes conforming markdown+frontmatter into
//! the vault is a valid producer.

/// The Curio adapter.
///
/// Consumes vanilla `curio.frontmatter.v1` notes and `curio.events.v1`
/// JSONL, validated against the vendored sha-pinned schemas under
/// `contracts/vendor/curio/`. Maps `curio_id` → `kp_id: curio:<id>`.
///
/// Boundary rules (binding, from `contracts/kp-note/v1.md`):
/// - `.curio/**` is Curio-owned; the plane never writes there.
/// - `.curio/manifest.json` is read as the write-ownership oracle.
/// - Events are tailed with rotation-aware `(file, line)` cursors and
///   deduped by `event_id`; behavioral events are never committed to git.
#[derive(Debug, Default)]
pub struct CurioAdapter {
    _private: (),
}

impl CurioAdapter {
    /// Create the adapter (stub — wiring lands with the ingest milestone).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}
