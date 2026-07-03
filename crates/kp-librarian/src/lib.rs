//! Knowledge Plane librarian — deterministic-first, zero-LLM baseline.
//!
//! Candidate set = notes since the last digest; score =
//! `cosine(note, now.md anchor) × exp(−age / half_life)`; top-k grouped by
//! tag/source; rendered as a digest note with links and extractive
//! one-line summaries; delivered as a `proposals/v1` proposal
//! (auto-applicable only when it purely ADDS files under the digest dir,
//! `kp_id: kp:<uuidv7>`). Digests are create-only and idempotent by date.
//!
//! An agent harness is an OPTIONAL prose enhancer riding the proposals
//! path — enabling it changes prose quality, never artifact shape. The
//! system is fully functional without it.

/// The deterministic digest engine (stub — lands with the librarian milestone).
#[derive(Debug, Default)]
pub struct Librarian {
    _private: (),
}

impl Librarian {
    /// Create the librarian.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}
