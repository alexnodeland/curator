//! Knowledge Plane Zotero producer (read-only at v1).
//!
//! Channel 1 — metadata via the Zotero Web API v3: `since=` delta polling
//! with `If-Modified-Since-Version`/`Last-Modified-Version` (304-aware),
//! `Link`-header pagination, `/deleted` tombstones. Items map to
//! kp-note/v1 files (`kp_id: zotero:<itemKey>`) inside a
//! `kp-zotero:managed` comment-marker region, so re-syncs update machine
//! content without clobbering user additions. All tests are
//! fixture-driven (hermetic).

pub mod api;
pub mod error;
pub mod item;
pub mod managed;
pub mod map;

pub use api::{ItemsDelta, ZoteroApi};
pub use error::ZoteroError;
pub use item::{Creator, Deleted, Fulltext, Item, ItemData, Tag};
pub use managed::{MANAGED_BEGIN, MANAGED_END, ManagedSplit, is_pristine, split_managed};
pub use map::{MappedNote, map_item, note_rel_path, render_managed};
