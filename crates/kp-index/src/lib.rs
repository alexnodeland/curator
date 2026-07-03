//! Knowledge Plane index.
//!
//! All derived retrieval state lives in ONE embedded SQLite file
//! (`index.db`): vectors (sqlite-vec), full-text (FTS5), and a relational
//! edge graph. The whole file is disposable — blue/green epoch rebuilds,
//! never migrations. Everything in this crate is INTERNAL (not a published
//! contract) and may change freely.

pub mod chunk;
pub mod db;
pub mod embed;
#[cfg(feature = "embed-onnx")]
pub mod embed_onnx;
pub mod epoch;
pub mod error;
pub mod query;
pub mod search;

pub use chunk::{Chunk, ChunkParams, chunk_text};
pub use db::{
    BehaviorDelta, BehaviorStats, Index, IndexMeta, IndexReader, NoteState, SCHEMA_VERSION,
};
pub use embed::{Embedder, HashEmbedder, embedder_from_config};
#[cfg(feature = "embed-onnx")]
pub use embed_onnx::FastEmbedder;
pub use epoch::{ChunkFn, EpochReport, EpochSource, build_epoch, build_epoch_from};
pub use error::IndexError;
pub use query::{NoteRecord, NoteSummary};
pub use search::SearchHit;
