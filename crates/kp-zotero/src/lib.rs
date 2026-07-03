//! Knowledge Plane Zotero producer (read-only at v1).
//!
//! Channel 1 — metadata via the Zotero Web API v3: `since=` delta polling
//! with `If-Modified-Since-Version`/`Last-Modified-Version` (304-aware),
//! `Link`-header pagination, `/deleted` tombstones. All tests are
//! fixture-driven (hermetic).

pub mod api;
pub mod error;
pub mod item;

pub use api::{ItemsDelta, ZoteroApi};
pub use error::ZoteroError;
pub use item::{Creator, Deleted, Fulltext, Item, ItemData, Tag};
