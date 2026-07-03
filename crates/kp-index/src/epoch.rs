//! Blue/green epoch rebuilds — never in-place migrations.
//!
//! An index epoch is a pure function of (embedding model + dims, chunker
//! version, normalization version). Any mismatch — new model, new schema,
//! new chunker — means [`build_epoch`]: write `index.db.next` COMPLETELY,
//! verify it, then atomically rename over `index.db`. A crash at any point
//! leaves the serving epoch untouched; there is no partially-migrated
//! state to recover, ever. Incremental ingest (same epoch) updates in
//! place via [`Index::upsert_note`].

use std::path::Path;

use kp_core::{KpConfig, Vault};

use crate::chunk::ChunkParams;
use crate::db::Index;
use crate::embed::Embedder;
use crate::error::IndexError;

/// What a finished epoch build did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpochReport {
    /// The epoch counter stamped into the new file.
    pub epoch: i64,
    /// Notes indexed from the vault.
    pub notes_indexed: usize,
    /// Notes skipped because they failed to parse (warned, not fatal — a
    /// single malformed producer file must not brick reindexing).
    pub notes_skipped: usize,
}

/// Build a complete new index epoch from the configured vault, verify it,
/// and atomically swap it in. Returns only after `index.db` IS the new
/// epoch (or an error, in which case the previous `index.db` — if any —
/// is untouched).
pub fn build_epoch(config: &KpConfig, embedder: &dyn Embedder) -> Result<EpochReport, IndexError> {
    let vault = Vault::open(config.vault_path())?;
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
    // A leftover .next is residue of a crashed build: discard, rebuild.
    remove_db_files(&next_path)?;

    // The epoch counter continues from the serving file when it is
    // readable; anything else (first build, corrupt file) restarts at 1.
    let epoch = crate::db::IndexReader::open(&live_path)
        .map(|r| r.meta().epoch + 1)
        .unwrap_or(1);

    let mut next = Index::create(&next_path, embedder, epoch)?;
    let params = ChunkParams::from_config(&config.index);
    let mut indexed = 0usize;
    let mut skipped = 0usize;
    let mut seen = std::collections::HashSet::new();
    let rels = vault.note_paths()?;
    for rel in &rels {
        match vault.read_note(rel) {
            Ok(note) => {
                // Two files claiming one identity is a producer bug; the
                // first (path-sorted) file wins deterministically, the
                // rest are surfaced, not silently merged.
                if !seen.insert(note.kp_id().to_string()) {
                    tracing::warn!(note = %rel, kp_id = %note.kp_id(),
                        "skipping note with duplicate kp_id in epoch build");
                    skipped += 1;
                    continue;
                }
                next.upsert_note(&note, embedder, params)?;
                indexed += 1;
            }
            Err(err) => {
                tracing::warn!(note = %rel, %err, "skipping unparseable note in epoch build");
                skipped += 1;
            }
        }
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

    // Clear the OLD epoch's sidecars before the swap — a stale index.db-wal
    // from the previous file must never be replayed into the new one.
    // (Single-writer batch discipline makes this safe; see db.rs.)
    remove_sidecars(&live_path)?;
    std::fs::rename(&next_path, &live_path).map_err(|source| IndexError::Io {
        path: live_path.clone(),
        source,
    })?;

    Ok(EpochReport {
        epoch,
        notes_indexed: indexed,
        notes_skipped: skipped,
    })
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
