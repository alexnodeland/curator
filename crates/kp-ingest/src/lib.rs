//! Knowledge Plane ingest — producers → vault/index.
//!
//! Producers integrate by ADAPTER, never by template: this crate consumes
//! a producer's own published formats and maps them onto `kp-note/v1`
//! identities. Anything that writes conforming markdown+frontmatter into
//! the vault is a valid producer.
//!
//! | module | role |
//! |---|---|
//! | [`walker`] | vault walk: dot-dirs skipped, `.kpignore` honored |
//! | [`curio`] | the Curio adapter: vendored-schema validation, `curio_id` → `curio:<id>`, managed split, manifest ownership oracle |
//! | [`events`] | rotation-aware `curio.events.v1` tail → behavior rollups |
//! | [`chunker`] | heading-aware markdown chunker (fences atomic) |
//! | [`ingest`] | the orchestration: walk → adapt → chunk → batch-embed → upsert → link → tail |

pub mod chunker;
pub mod curio;
pub mod error;
pub mod events;
pub mod ingest;
pub mod walker;

pub use chunker::chunk_markdown;
pub use curio::{AdaptedCurio, CurioAdapt, CurioAdapter, CurioEvent, CurioManifest, ManagedSplit};
pub use error::IngestError;
pub use events::{EVENTS_CONSUMER, TailReport, tail_events};
pub use ingest::{IngestReport, LinkKind, RawLink, RebuildReport, extract_links, ingest, rebuild};
pub use walker::{KpIgnore, WalkReport, WalkedNote, walk_vault};
