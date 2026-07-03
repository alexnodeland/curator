//! Blue/green epoch rebuilds — never in-place migrations.
//!
//! An index epoch's RETRIEVAL state is a pure function of (embedding
//! model + dims, chunker version, normalization version). Any mismatch —
//! new model, new schema, new chunker — means [`build_epoch`]: write
//! `index.db.next` COMPLETELY, verify it, then atomically rename over
//! `index.db`. Consumer/operational state (cursors, event dedupe,
//! behavior rollups, digest log) is NOT derivable from the vault and is
//! carried forward from the serving epoch instead
//! ([`Index::copy_consumer_state_from`]). A crash at any point leaves the
//! serving epoch untouched; there is no partially-migrated state to
//! recover, ever. Incremental ingest (same epoch) updates in place via
//! [`Index::upsert_note`]. The whole build + swap holds the index writer
//! lock (see db.rs) — concurrent writers are refused, never raced.

use std::path::Path;

use kp_core::note::Note;
use kp_core::{KpConfig, Vault};

use crate::chunk::{Chunk, ChunkParams, chunk_text};
use crate::db::Index;
use crate::embed::Embedder;
use crate::error::IndexError;

/// A pluggable chunking function: callers with richer chunkers (e.g. the
/// heading-aware markdown chunker in kp-ingest, which this crate must not
/// depend on) inject theirs so an epoch rebuild produces exactly the same
/// chunks as incremental ingest.
pub type ChunkFn<'a> = &'a dyn Fn(&str, ChunkParams) -> Vec<Chunk>;

/// The note corpus an epoch build indexes. Richer pipelines (kp-ingest:
/// `.kpignore`, the Curio adapter's identity mapping) hand in their
/// prepared view so a rebuild reproduces EXACTLY what incremental ingest
/// would index — same notes, same identities.
#[derive(Debug, Default)]
pub struct EpochSource {
    /// Parsed (and possibly adapted) notes, path-sorted.
    pub notes: Vec<Note>,
    /// Files the source already warned about and dropped (parse failures,
    /// producer-schema violations) — reported, not fatal.
    pub skipped: usize,
}

impl EpochSource {
    /// The default source: every parseable note in the vault, as-is.
    /// Parse failures are warned + counted, never fatal.
    pub fn from_vault(vault: &Vault) -> Result<Self, IndexError> {
        let mut source = Self::default();
        for rel in vault.note_paths()? {
            match vault.read_note(&rel) {
                Ok(note) => source.notes.push(note),
                Err(err) => {
                    tracing::warn!(note = %rel, %err, "skipping unparseable note in epoch build");
                    source.skipped += 1;
                }
            }
        }
        Ok(source)
    }
}

/// What a finished epoch build did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpochReport {
    /// The epoch counter stamped into the new file.
    pub epoch: i64,
    /// Notes indexed from the vault.
    pub notes_indexed: usize,
    /// Notes skipped (parse failures, duplicate identities) — warned, not
    /// fatal: a single malformed producer file must not brick reindexing.
    pub notes_skipped: usize,
}

/// Build a complete new index epoch from the configured vault, verify it,
/// and atomically swap it in. Returns only after `index.db` IS the new
/// epoch (or an error, in which case the previous `index.db` — if any —
/// is untouched). Chunks with this crate's generic token-window chunker
/// over the raw vault corpus; use [`build_epoch_from`] to inject a richer
/// chunker and note source.
pub fn build_epoch(config: &KpConfig, embedder: &dyn Embedder) -> Result<EpochReport, IndexError> {
    let vault = Vault::open(config.vault_path())?;
    let source = EpochSource::from_vault(&vault)?;
    build_epoch_from(config, embedder, &chunk_text, source)
}

/// [`build_epoch`] with an injected chunking function and note corpus.
pub fn build_epoch_from(
    config: &KpConfig,
    embedder: &dyn Embedder,
    chunker: ChunkFn<'_>,
    source: EpochSource,
) -> Result<EpochReport, IndexError> {
    let live_path = config.index_path();
    let next_path = {
        let mut p = live_path.clone().into_os_string();
        p.push(".next");
        std::path::PathBuf::from(p)
    };
    if let Some(parent) = live_path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| IndexError::Io {
            path: parent.to_owned(),
            source,
        })?;
    }
    // Take the LIVE index's writer lock for the whole build + swap: a
    // concurrent writer (kp ingest mid-run) must never have its WAL
    // deleted out from under it by the swap — that would silently discard
    // its entire run while it reports success.
    let _live_lock = crate::db::acquire_writer_lock(&live_path)?;
    // A leftover .next is residue of a crashed build: discard, rebuild.
    remove_db_files(&next_path)?;

    // The epoch counter continues from the serving file when it is
    // readable; anything else (first build, corrupt file, older schema)
    // restarts at 1 — and only a readable same-schema file donates its
    // consumer state below.
    let prev_meta = crate::db::IndexReader::open(&live_path)
        .ok()
        .map(|r| r.meta().clone());
    let epoch = prev_meta.as_ref().map_or(1, |m| m.epoch + 1);

    let mut next = Index::create(&next_path, embedder, epoch)?;
    let params = ChunkParams::from_config(&config.index);
    let mut indexed = 0usize;
    let mut skipped = source.skipped;
    let mut seen = std::collections::HashSet::new();
    for note in &source.notes {
        // Two files claiming one identity is a producer bug; the first
        // (path-sorted) file wins deterministically, the rest are
        // surfaced, not silently merged.
        if !seen.insert(note.kp_id().to_string()) {
            tracing::warn!(note = %note.rel_path, kp_id = %note.kp_id(),
                "skipping note with duplicate kp_id in epoch build");
            skipped += 1;
            continue;
        }
        let chunks = chunker(&note.body, params);
        let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
        let vectors = embedder.embed(&texts)?;
        next.upsert_note_prechunked(note, embedder, &chunks, &vectors)?;
        indexed += 1;
    }

    // Consumer state is not derivable from the vault — carry it forward
    // from the serving epoch (cursors incl. producer cursors like the
    // Zotero library version, event dedupe, behavior rollups, digest
    // log). Without this, a routine `kp reindex` would drop e.g. the
    // Zotero cursor, and tombstones that fired before the rebuild would
    // be missed forever (a fresh sync starts past them).
    if prev_meta.is_some() {
        next.copy_consumer_state_from(&live_path)?;
    }

    // Verify BEFORE the swap: structural integrity + completeness. A next
    // file that fails verification never replaces the serving epoch.
    next.integrity_check()?;
    let count = next.note_count()?;
    if count != indexed as i64 {
        return Err(IndexError::EpochVerification(format!(
            "completeness check failed: indexed {indexed} notes but the file holds {count}"
        )));
    }
    // Close cleanly: checkpoints the WAL and removes -wal/-shm sidecars,
    // so the rename moves ONE self-contained file.
    next.close()?;

    // Retire the OLD epoch's sidecars before the swap — a stale
    // index.db-wal from the previous file must never be replayed into the
    // new one. Checkpoint FIRST (TRUNCATE folds any committed WAL frames
    // into the main file and empties the WAL), so a crash between the
    // sidecar removal and the rename leaves the previous epoch serving
    // with ALL of its committed transactions — never rolled back to its
    // last checkpoint. The writer lock held above makes this safe against
    // concurrent writers; readers re-open per operation.
    checkpoint_wal(&live_path);
    remove_sidecars(&live_path)?;
    std::fs::rename(&next_path, &live_path).map_err(|source| IndexError::Io {
        path: live_path.clone(),
        source,
    })?;
    // Tidy the .next writer-lock file (its lock was released when the
    // build handle closed). Best-effort — a leftover empty file is inert.
    let _ = std::fs::remove_file(crate::db::writer_lock_path(&next_path));

    Ok(EpochReport {
        epoch,
        notes_indexed: indexed,
        notes_skipped: skipped,
    })
}

/// Fold any committed WAL frames of the db at `path` into its main file
/// (`PRAGMA wal_checkpoint(TRUNCATE)`). Best-effort: a missing, corrupt,
/// or non-WAL file is skipped — the caller removes the sidecars anyway.
fn checkpoint_wal(path: &Path) {
    if !path.exists() {
        return;
    }
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return;
    };
    if let Err(err) = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())) {
        tracing::warn!(%err, path = %path.display(),
            "could not checkpoint the previous epoch's WAL before the swap");
    }
}

fn remove_db_files(path: &Path) -> Result<(), IndexError> {
    remove_if_present(path)?;
    remove_sidecars(path)
}

fn remove_sidecars(path: &Path) -> Result<(), IndexError> {
    for suffix in ["-wal", "-shm"] {
        let mut p = path.to_owned().into_os_string();
        p.push(suffix);
        remove_if_present(Path::new(&p))?;
    }
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<(), IndexError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(IndexError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}
