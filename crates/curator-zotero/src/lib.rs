//! Knowledge Plane Zotero producer (read-only at v1).
//!
//! Two channels:
//! 1. **Metadata** — the Zotero Web API v3: `since=` delta polling with
//!    `If-Modified-Since-Version`/`Last-Modified-Version` (304-aware),
//!    `Link`-header pagination, `/deleted` tombstones. Items map to
//!    kp-note/v1 files (`kp_id: zotero:<itemKey>`) inside a
//!    `kp-zotero:managed` comment-marker region, so re-syncs update
//!    machine content without clobbering user additions.
//! 2. **Fulltext** — the official `/items/{key}/fulltext` endpoint first;
//!    optionally (config `webdav_fallback`) a small CRC-verified WebDAV
//!    `.prop`/`.zip` shim for self-hosted attachment stores.
//!
//! Identity is strictly `zotero:<itemKey>` — a citekey or title change is
//! an update to the same note, never a duplicate. Tombstones remove only
//! fully machine-owned files; anything the user edited moves to
//! `.kp/trash/`. The library-version cursor persists in curator-index's
//! cursors table. All tests are fixture-driven (hermetic).

pub mod api;
pub mod error;
pub mod item;
pub mod managed;
pub mod map;
pub mod sync;
pub mod webdav;

pub use api::{ItemsDelta, ZoteroApi};
pub use error::ZoteroError;
pub use item::{Creator, Deleted, Fulltext, Item, ItemData, Tag};
pub use managed::{MANAGED_BEGIN, MANAGED_END, ManagedSplit, is_pristine, split_managed};
pub use map::{MappedNote, map_item, note_rel_path, render_managed};
pub use sync::{CURSOR_CONSUMER, CURSOR_FILE, SyncOptions, SyncReport, sync};
pub use webdav::{PropInfo, ShimCaps, WebDavShim, parse_prop};
