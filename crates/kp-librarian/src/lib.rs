//! Knowledge Plane librarian — deterministic-first, zero-LLM baseline.
//!
//! Two engines live here:
//!
//! - **Proposals** ([`proposals`]): the read/review/apply side of
//!   `proposals/v1` (creation is kp-core's `create_proposal`). The apply
//!   validator is deterministic and forge-free — path safety, the Curio
//!   ownership oracle (`.curio/manifest.json` via kp-ingest) + managed
//!   regions, strict clean patch application, identity uniqueness — and
//!   stamps `open → applied | rejected` into `proposal.json`.
//! - **Digest** ([`digest`]): candidate set = notes ingested or active
//!   since the last digest; score = `cosine(note centroid, now.md anchor)
//!   × exp(−age / half_life) × behavior boost`; top-k grouped by
//!   tag/source; rendered as a digest note with wikilinks, extractive
//!   one-line summaries, why-surfaced notes and a quiet-items tail;
//!   delivered as a `proposals/v1` proposal (auto-applicable only when it
//!   purely ADDS files under the digest dir with `kp_id: kp:<uuidv7>`).
//!   Digests are create-only and idempotent by date, and byte-identical
//!   for identical inputs (the clock is injected).
//!
//! An agent harness is an OPTIONAL prose enhancer riding the proposals
//! path (`docs/design/enhancer.md`) — enabling it changes prose quality,
//! never artifact shape. The system is fully functional without it.

pub mod digest;
pub mod patch;
pub mod proposals;
pub mod uuid7;

pub use digest::{DigestError, DigestReport, run_digest};
pub use patch::{FilePatch, Hunk, HunkLine, PatchError, apply_file_patch, parse_patch};
pub use proposals::{ApplyError, ApplyReport, apply_proposal, auto_applicable, render_review};
pub use uuid7::{is_uuid7, mint_uuid7};
