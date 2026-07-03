//! `kp zotero sync` — the two-channel orchestration.
//!
//! One run: delta-poll the Web API (channel 1) → resolve changed
//! top-level items (+ their attachments) → fulltext pass (channel 2:
//! official `/fulltext` first, WebDAV shim fallback when configured) →
//! upsert kp-note/v1 files into the configured vault dir (managed-region
//! merge, user zones preserved) → apply tombstones (pristine files are
//! removed, user-edited files move to `.kp/trash/` — never deleted) →
//! persist the library-version cursor in kp-index.
//!
//! Disabled is a first-class clean outcome: `[zotero] enabled = false`, an
//! unset/empty API key env, or an empty `user_id` all return a report with
//! `enabled: false` and a reason — no network, no error.

use std::collections::BTreeMap;

use kp_core::note::Frontmatter;
use kp_core::{KpConfig, Note, Vault};
use kp_index::Index;

use crate::api::ZoteroApi;
use crate::error::ZoteroError;
use crate::item::Item;
use crate::managed::{compose_body, is_pristine, split_managed};
use crate::map::{MappedNote, map_item, note_rel_path};
use crate::webdav::{ShimCaps, WebDavShim};

/// The cursors-table consumer name for this producer.
pub const CURSOR_CONSUMER: &str = "zotero";
/// The cursors-table "file" key holding the library version.
pub const CURSOR_FILE: &str = "library-version";
/// Where tombstoned-but-user-edited notes go (vault-relative).
pub const TRASH_DIR: &str = ".kp/trash";

/// Knobs that are the producer's own (NOT part of kp-config/v1 — the
/// contract's `[zotero]` table is implemented verbatim in `kp-core`;
/// these defaults are overridable per-invocation, e.g. by CLI flags).
#[derive(Debug, Clone)]
pub struct SyncOptions {
    /// Vault-relative directory the notes land in.
    pub notes_dir: String,
    /// Run the fulltext pass at all.
    pub fulltext: bool,
    /// Truncation cap for the `## Fulltext` section, in characters.
    pub fulltext_max_chars: usize,
    /// WebDAV shim size caps.
    pub caps: ShimCaps,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            notes_dir: "zotero".to_owned(),
            fulltext: true,
            fulltext_max_chars: 20_000,
            caps: ShimCaps::default(),
        }
    }
}

/// What one sync run did — the `--json` summary of `kp zotero sync`.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SyncReport {
    /// `false` = cleanly disabled (see `disabled_reason`), nothing ran.
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
    /// The library answered 304 — nothing changed since the cursor.
    pub not_modified: bool,
    /// The cursor before the run (`null` on the initial sync).
    pub version_before: Option<i64>,
    /// The cursor persisted after the run.
    pub version_after: Option<i64>,
    /// Item objects in the delta (all types).
    pub fetched: usize,
    /// Notes created or updated in the vault.
    pub upserted: usize,
    /// Notes whose rendered content was byte-identical.
    pub unchanged: usize,
    /// Delta objects not mapped (child notes, annotations, unmergeable
    /// files — each with a warning).
    pub skipped: usize,
    /// Tombstoned item keys reported by `/deleted`.
    pub tombstones: usize,
    /// Pristine (fully machine-owned) note files removed.
    pub deleted_files: usize,
    /// User-edited note files moved to `.kp/trash/` instead of deleted.
    pub trashed_files: usize,
    /// Items whose note gained a `## Fulltext` section this run.
    pub fulltext_added: usize,
    /// New/changed items with no retrievable fulltext on any channel.
    pub fulltext_missing: usize,
    pub warnings: Vec<String>,
}

/// Run one sync. The `index` handle persists the version cursor (and
/// drops rows for tombstoned notes); vault writes go through `Vault`'s
/// safety guarantees.
pub fn sync(
    config: &KpConfig,
    index: &mut Index,
    options: &SyncOptions,
) -> Result<SyncReport, ZoteroError> {
    let mut report = SyncReport::default();

    if !config.zotero.enabled {
        report.disabled_reason = Some("[zotero] enabled = false".to_owned());
        return Ok(report);
    }
    let Some(api_key) = config.zotero.api_key() else {
        report.disabled_reason = Some(format!(
            "API key env {:?} unset or empty — Zotero channel disabled",
            config.zotero.api_key_env
        ));
        return Ok(report);
    };
    if config.zotero.user_id.trim().is_empty() {
        report.disabled_reason = Some("[zotero] user_id is empty".to_owned());
        return Ok(report);
    }
    report.enabled = true;

    let vault = Vault::open(config.vault_path())?;
    let api = ZoteroApi::new(&config.zotero.api_base, &config.zotero.user_id, &api_key);

    let since = index.cursor(CURSOR_CONSUMER, CURSOR_FILE)?;
    report.version_before = since;

    // Channel 1: the metadata delta.
    let delta = api.items_since(since)?;
    report.fetched = delta.items.len();
    if delta.not_modified {
        report.not_modified = true;
        report.version_after = since;
        return Ok(report);
    }

    // Partition the delta: top-level mappable items vs. changed
    // attachments (whose parents must re-render even when unchanged
    // themselves) vs. object types this producer never maps.
    let mut parents: BTreeMap<String, Item> = BTreeMap::new();
    let mut orphan_attachment_parents: Vec<String> = Vec::new();
    for item in delta.items.clone() {
        if item.is_attachment() {
            if item.is_top_level() {
                report.skipped += 1; // standalone attachments have no note
            } else {
                orphan_attachment_parents.push(item.data.parent_item.clone());
            }
        } else if item.is_note_or_annotation() || !item.is_top_level() {
            report.skipped += 1;
        } else {
            parents.insert(item.key().to_owned(), item);
        }
    }
    // A changed attachment whose parent is not in the delta still dirties
    // the parent's note (attachment list + fulltext): fetch the parent.
    for parent_key in orphan_attachment_parents {
        if parents.contains_key(&parent_key) {
            continue;
        }
        match api.item(&parent_key)? {
            Some(parent)
                if parent.is_top_level()
                    && !parent.is_attachment()
                    && !parent.is_note_or_annotation() =>
            {
                parents.insert(parent.key().to_owned(), parent);
            }
            Some(_) => report.skipped += 1,
            // Parent already gone server-side; its tombstone handles it.
            None => {}
        }
    }

    // Per changed item: attachments, fulltext (channel 2), render, upsert.
    for (key, item) in &parents {
        let attachments: Vec<Item> = api
            .children(key)?
            .into_iter()
            .filter(Item::is_attachment)
            .collect();

        let mut fulltext: Option<String> = None;
        if options.fulltext {
            for att in &attachments {
                if let Some(ft) = api.fulltext(att.key())? {
                    fulltext = Some(ft.content);
                    break;
                }
            }
            if fulltext.is_none()
                && config.zotero.webdav_fallback
                && !config.zotero.webdav_url.trim().is_empty()
            {
                let shim = WebDavShim::new(api.http(), &config.zotero.webdav_url, options.caps);
                for att in &attachments {
                    match shim.fetch_fulltext(att.key()) {
                        Ok(Some(text)) => {
                            fulltext = Some(text);
                            break;
                        }
                        Ok(None) => {}
                        Err(e) => report
                            .warnings
                            .push(format!("webdav fulltext for {}: {e}", att.key())),
                    }
                }
            }
            if fulltext.is_some() {
                report.fulltext_added += 1;
            } else {
                report.fulltext_missing += 1;
            }
        }

        let mapped = map_item(
            item,
            &attachments,
            fulltext.as_deref(),
            options.fulltext_max_chars,
        );
        let rel = note_rel_path(&options.notes_dir, key);
        match upsert_note(&vault, &rel, &mapped)? {
            Upsert::Written => report.upserted += 1,
            Upsert::Unchanged => report.unchanged += 1,
            Upsert::Skipped(reason) => {
                report.skipped += 1;
                report.warnings.push(format!("{rel}: {reason}"));
            }
        }
    }

    // Tombstones — only meaningful once a cursor exists (an initial sync
    // has nothing local to tombstone).
    if let Some(since) = since {
        let deleted = api.deleted_since(since)?;
        report.tombstones = deleted.items.len();
        for key in &deleted.items {
            match remove_tombstoned(&vault, &options.notes_dir, key)? {
                Removal::Absent => {}
                Removal::Deleted => {
                    report.deleted_files += 1;
                    index.remove_note(&format!("zotero:{key}"))?;
                }
                Removal::Trashed(dest) => {
                    report.trashed_files += 1;
                    report
                        .warnings
                        .push(format!("{key}: user-edited note moved to {dest}"));
                    index.remove_note(&format!("zotero:{key}"))?;
                }
            }
        }
    }

    index.set_cursor(CURSOR_CONSUMER, CURSOR_FILE, delta.version)?;
    report.version_after = Some(delta.version);
    Ok(report)
}

/// One upsert outcome.
#[derive(Debug)]
enum Upsert {
    Written,
    Unchanged,
    Skipped(String),
}

/// Write or merge one mapped note. Fresh files get empty user zones; on
/// re-sync only the managed region and the machine frontmatter fields are
/// replaced — user zones and extra frontmatter keys ride along untouched.
/// A file this producer cannot merge safely (markers gone, foreign
/// frontmatter, identity mismatch) is never overwritten.
fn upsert_note(vault: &Vault, rel: &str, mapped: &MappedNote) -> Result<Upsert, ZoteroError> {
    let existing_raw = match vault.read(rel) {
        Ok(raw) => raw,
        // Not there yet (or unreadable as text — resolve errors surface
        // on the write instead): fresh file.
        Err(_) => {
            vault.write_atomic(rel, &mapped.fresh_content())?;
            return Ok(Upsert::Written);
        }
    };
    let Ok(existing) = Note::parse(rel, &existing_raw) else {
        return Ok(Upsert::Skipped(
            "existing file does not parse as a note — leaving it untouched".to_owned(),
        ));
    };
    let Frontmatter::Kp(existing_fm) = &existing.frontmatter else {
        return Ok(Upsert::Skipped(
            "existing file lacks kp-note frontmatter — user owns it now".to_owned(),
        ));
    };
    if existing_fm.kp_id != mapped.frontmatter.kp_id {
        return Ok(Upsert::Skipped(format!(
            "identity collision: file carries {} but item is {}",
            existing_fm.kp_id, mapped.frontmatter.kp_id
        )));
    }
    let Some(split) = split_managed(&existing.body) else {
        return Ok(Upsert::Skipped(
            "managed markers missing — user owns this file now".to_owned(),
        ));
    };

    let mut fm = mapped.frontmatter.clone();
    // Extra frontmatter keys are the user's (kp-note binding rule 3).
    fm.extra = existing_fm.extra.clone();
    let merged = Note {
        rel_path: rel.to_owned(),
        frontmatter: Frontmatter::Kp(fm),
        body: compose_body(&split.before, &mapped.managed, &split.after),
    }
    .to_markdown();

    if merged == existing_raw {
        return Ok(Upsert::Unchanged);
    }
    vault.write_atomic(rel, &merged)?;
    Ok(Upsert::Written)
}

/// One tombstone outcome.
#[derive(Debug, PartialEq, Eq)]
enum Removal {
    Absent,
    Deleted,
    Trashed(String),
}

/// Apply one tombstone: pristine (fully machine-owned) files are removed;
/// anything the user touched moves to `.kp/trash/` — NEVER deleted.
fn remove_tombstoned(vault: &Vault, notes_dir: &str, key: &str) -> Result<Removal, ZoteroError> {
    let rel = note_rel_path(notes_dir, key);
    let Ok(raw) = vault.read(&rel) else {
        return Ok(Removal::Absent);
    };
    let path = vault.resolve(&rel)?;
    if is_pristine(&rel, &raw) {
        std::fs::remove_file(&path).map_err(|source| ZoteroError::Io { path, source })?;
        return Ok(Removal::Deleted);
    }
    // Find a free trash slot: KEY.md, KEY-1.md, KEY-2.md, ...
    let mut dest = format!("{TRASH_DIR}/{key}.md");
    let mut n = 0;
    while vault.resolve(&dest)?.exists() {
        n += 1;
        dest = format!("{TRASH_DIR}/{key}-{n}.md");
    }
    vault.write_atomic(&dest, &raw)?;
    std::fs::remove_file(&path).map_err(|source| ZoteroError::Io { path, source })?;
    Ok(Removal::Trashed(dest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managed::{MANAGED_BEGIN, MANAGED_END};

    fn vault_in(dir: &std::path::Path) -> Vault {
        let root = dir.join("vault");
        std::fs::create_dir_all(&root).expect("mkdir");
        Vault::open(&root).expect("open")
    }

    fn mapped(key: &str) -> MappedNote {
        let mut item = Item::default();
        item.data.key = key.to_owned();
        item.data.item_type = "book".to_owned();
        item.data.title = "A Book".to_owned();
        map_item(&item, &[], None, 1000)
    }

    #[test]
    fn fresh_upsert_then_unchanged_then_merge_preserves_user_zone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = vault_in(dir.path());
        let m = mapped("KEY00001");

        // Fresh write.
        assert!(matches!(
            upsert_note(&vault, "zotero/KEY00001.md", &m).expect("upsert"),
            Upsert::Written
        ));
        // Same content → Unchanged.
        assert!(matches!(
            upsert_note(&vault, "zotero/KEY00001.md", &m).expect("upsert"),
            Upsert::Unchanged
        ));

        // User appends below the region + adds a frontmatter key.
        let raw = vault.read("zotero/KEY00001.md").expect("read");
        let edited = raw.replace("title:", "rating: 5\ntitle:").replace(
            &format!("{MANAGED_END}\n"),
            &format!("{MANAGED_END}\n\nMy margin notes.\n"),
        );
        vault
            .write_atomic("zotero/KEY00001.md", &edited)
            .expect("write");

        // Re-sync with changed machine content.
        let mut item = Item::default();
        item.data.key = "KEY00001".to_owned();
        item.data.item_type = "book".to_owned();
        item.data.title = "A Book, 2nd ed.".to_owned();
        let m2 = map_item(&item, &[], None, 1000);
        assert!(matches!(
            upsert_note(&vault, "zotero/KEY00001.md", &m2).expect("upsert"),
            Upsert::Written
        ));
        let merged = vault.read("zotero/KEY00001.md").expect("read");
        assert!(merged.contains("A Book, 2nd ed."), "{merged}");
        assert!(merged.contains("My margin notes."), "user zone eaten");
        assert!(merged.contains("rating: 5"), "extra frontmatter key eaten");
    }

    #[test]
    fn unmergeable_files_are_never_overwritten() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = vault_in(dir.path());

        // Markers stripped by the user.
        vault
            .write_atomic(
                "zotero/KEYAAAA1.md",
                "---\nkp_id: \"zotero:KEYAAAA1\"\nkp_schema: kp-note/v1\ntitle: Mine\n---\nrewritten\n",
            )
            .expect("write");
        let m = mapped("KEYAAAA1");
        assert!(matches!(
            upsert_note(&vault, "zotero/KEYAAAA1.md", &m).expect("upsert"),
            Upsert::Skipped(_)
        ));
        assert!(
            vault
                .read("zotero/KEYAAAA1.md")
                .expect("read")
                .contains("rewritten")
        );

        // Identity collision.
        vault
            .write_atomic(
                "zotero/KEYBBBB1.md",
                &format!(
                    "---\nkp_id: \"zotero:OTHER111\"\nkp_schema: kp-note/v1\ntitle: X\n---\n{MANAGED_BEGIN}x{MANAGED_END}\n"
                ),
            )
            .expect("write");
        let m = mapped("KEYBBBB1");
        assert!(matches!(
            upsert_note(&vault, "zotero/KEYBBBB1.md", &m).expect("upsert"),
            Upsert::Skipped(_)
        ));

        // Foreign frontmatter.
        vault
            .write_atomic("zotero/KEYCCCC1.md", "---\nother: tool\n---\nbody\n")
            .expect("write");
        let m = mapped("KEYCCCC1");
        assert!(matches!(
            upsert_note(&vault, "zotero/KEYCCCC1.md", &m).expect("upsert"),
            Upsert::Skipped(_)
        ));
    }

    #[test]
    fn tombstone_deletes_pristine_and_trashes_edited() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = vault_in(dir.path());

        // Pristine machine file → deleted outright.
        let m = mapped("KEYDDDD1");
        upsert_note(&vault, "zotero/KEYDDDD1.md", &m).expect("upsert");
        assert_eq!(
            remove_tombstoned(&vault, "zotero", "KEYDDDD1").expect("rm"),
            Removal::Deleted
        );
        assert!(vault.read("zotero/KEYDDDD1.md").is_err());

        // User-edited file → trash, never delete.
        let m = mapped("KEYEEEE1");
        upsert_note(&vault, "zotero/KEYEEEE1.md", &m).expect("upsert");
        let raw = vault.read("zotero/KEYEEEE1.md").expect("read");
        vault
            .write_atomic(
                "zotero/KEYEEEE1.md",
                &format!("{raw}\nirreplaceable thoughts\n"),
            )
            .expect("write");
        assert_eq!(
            remove_tombstoned(&vault, "zotero", "KEYEEEE1").expect("rm"),
            Removal::Trashed(".kp/trash/KEYEEEE1.md".to_owned())
        );
        assert!(vault.read("zotero/KEYEEEE1.md").is_err());
        assert!(
            vault
                .read(".kp/trash/KEYEEEE1.md")
                .expect("trashed copy exists")
                .contains("irreplaceable thoughts")
        );

        // A second trash of the same key finds a free slot.
        vault
            .write_atomic("zotero/KEYEEEE1.md", "not even a kp note")
            .expect("write");
        assert_eq!(
            remove_tombstoned(&vault, "zotero", "KEYEEEE1").expect("rm"),
            Removal::Trashed(".kp/trash/KEYEEEE1-1.md".to_owned())
        );

        // Absent file → no-op.
        assert_eq!(
            remove_tombstoned(&vault, "zotero", "KEYFFFF1").expect("rm"),
            Removal::Absent
        );
    }
}
