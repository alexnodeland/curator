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
pub mod error;
pub mod search;

pub use chunk::{Chunk, ChunkParams, chunk_text};
pub use db::{Index, IndexMeta, IndexReader, SCHEMA_VERSION};
pub use embed::{Embedder, HashEmbedder};
pub use error::IndexError;
pub use search::SearchHit;
