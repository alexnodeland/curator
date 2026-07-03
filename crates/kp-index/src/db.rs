//! The ONE embedded index database.
//!
//! All derived retrieval state lives in a single SQLite file: vectors
//! (sqlite-vec), full-text (FTS5), the relational edge graph, ingest
//! cursors, and behavioral rollups. The whole file is disposable —
//! blue/green epoch rebuilds (see [`crate::epoch`]), never migrations.
//!
//! ## Writer discipline
//!
//! [`Index`] is THE writer handle — exactly one per process — and
//! [`IndexReader`] wraps separate read-only connections. A server-grade
//! design would funnel writes through a dedicated writer thread + channel
//! queue so concurrent request handlers never collide on `SQLITE_BUSY`;
//! this is deliberately NOT that, because the KP write workload is batch
//! CLI commands (`kp ingest`, `kp reindex`) — one process, one command,
//! holding the writer for the life of the run, with no concurrent request
//! fan-in to arbitrate. WAL mode keeps concurrent READERS (per-session
//! stdio MCP servers) unblocked while a batch write is in flight, and a
//! busy timeout absorbs the rare overlap of two CLI invocations.

use std::path::{Path, PathBuf};
use std::sync::Once;

use kp_core::note::{Frontmatter, Note};
use rusqlite::{Connection, OpenFlags, params};

use crate::chunk::{Chunk, ChunkParams, chunk_text};
use crate::embed::Embedder;
use crate::error::IndexError;

/// The on-disk schema version. Bumping it is an EPOCH event: readers of a
/// different version refuse the file and demand a rebuild — there are no
/// in-place migrations, ever.
///
/// v2: added the `seen_events` dedupe table for the Curio events tail.
pub const SCHEMA_VERSION: i64 = 2;

/// Register sqlite-vec for every connection this process opens.
fn register_sqlite_vec() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: sqlite3_auto_extension is the C API's documented hook for
        // per-connection extension init, and sqlite3_vec_init is exactly the
        // `(db, pzErrMsg, pApi) -> int` entry point it expects; sqlite-vec
        // only exports it as a raw pointer, so the cast cannot be avoided
        // and rusqlite offers no safe wrapper. Registered exactly once,
        // process-wide, before any connection opens. This is the single
        // unsafe block in the workspace (see Cargo.toml [lints]).
        #[allow(unsafe_code)]
        unsafe {
            type InitFn = unsafe extern "C" fn(
                *mut rusqlite::ffi::sqlite3,
                *mut *mut std::os::raw::c_char,
                *const rusqlite::ffi::sqlite3_api_routines,
            ) -> std::os::raw::c_int;
            let init: InitFn = std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ());
            rusqlite::ffi::sqlite3_auto_extension(Some(init));
        }
    });
}

fn configure(conn: &Connection) -> Result<(), IndexError> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
}

/// The `meta` row: what epoch this file is and what produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexMeta {
    pub schema_version: i64,
    /// Embedder id every vector in this file came from.
    pub embedder_id: String,
    /// Vector dimensionality of `vec_chunks`.
    pub dims: usize,
    /// Monotonic epoch counter (bumped by every blue/green rebuild).
    pub epoch: i64,
    /// RFC 3339 build timestamp.
    pub built_at: String,
}

/// A note's indexed change-detection state: where it was and what change
/// token it carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteState {
    /// Vault-relative path as indexed.
    pub path: String,
    /// The indexed change token (`sha256:<hex>`), when one was recorded.
    pub checksum: Option<String>,
}

/// One behavioral event's effect on a note's rollup row (see
/// [`Index::apply_behavior`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BehaviorDelta {
    /// Added to `opened_count`.
    pub opened_delta: i64,
    /// `Some(v)` sets the starred flag (negation events set `false`).
    pub starred: Option<bool>,
    /// `Some(v)` sets the read-later flag (negation events set `false`).
    pub read_later: Option<bool>,
    /// Candidate for `last_activity` (kept monotonically maximal).
    pub activity_ts: Option<String>,
}

/// A note's behavioral rollup as stored in the `behavior` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BehaviorStats {
    pub opened_count: i64,
    pub starred: bool,
    pub read_later: bool,
    pub last_activity: Option<String>,
}

/// The single writer handle (see module docs for the discipline).
#[derive(Debug)]
pub struct Index {
    pub(crate) conn: Connection,
    path: PathBuf,
    meta: IndexMeta,
}

/// A read-only connection for retrieval. Cheap to open; open one per
/// reading context (each stdio MCP server session, each search command).
#[derive(Debug)]
pub struct IndexReader {
    pub(crate) conn: Connection,
    pub(crate) meta: IndexMeta,
}

impl Index {
    /// Create a brand-new index file with the full schema, stamped with
    /// the embedder's id/dims and the given epoch. Fails if `path` exists.
    pub fn create(
        path: impl AsRef<Path>,
        embedder: &dyn Embedder,
        epoch: i64,
    ) -> Result<Self, IndexError> {
        register_sqlite_vec();
        let path = path.as_ref().to_owned();
        if path.exists() {
            return Err(IndexError::Io {
                path,
                source: std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "index file already exists (epoch builds write index.db.next)",
                ),
            });
        }
        let conn = Connection::open(&path)?;
        configure(&conn)?;
        create_schema(&conn, embedder.dims())?;
        conn.execute(
            "INSERT INTO meta (id, schema_version, embedder_model, dims, epoch, built_at)
             VALUES (1, ?1, ?2, ?3, ?4, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
            params![SCHEMA_VERSION, embedder.id(), embedder.dims() as i64, epoch],
        )?;
        let meta = read_meta(&conn, &path)?;
        Ok(Self { conn, path, meta })
    }

    /// Open an existing index for writing. The supplied embedder MUST be
    /// the one the index was built with — id and dims are checked against
    /// the meta row, and a mismatch demands an epoch rebuild.
    pub fn open(path: impl AsRef<Path>, embedder: &dyn Embedder) -> Result<Self, IndexError> {
        register_sqlite_vec();
        let path = path.as_ref().to_owned();
        if !path.exists() {
            return Err(IndexError::Missing(path));
        }
        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        configure(&conn)?;
        let meta = read_meta(&conn, &path)?;
        check_meta(&meta, embedder)?;
        Ok(Self { conn, path, meta })
    }

    /// The meta row this handle validated at open/create.
    #[must_use]
    pub fn meta(&self) -> &IndexMeta {
        &self.meta
    }

    /// The database file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Open a separate read-only connection to the same file.
    pub fn reader(&self) -> Result<IndexReader, IndexError> {
        IndexReader::open(&self.path)
    }

    /// Insert or replace a note: metadata row (FTS rides the triggers),
    /// then re-chunk and re-embed the body. Incremental — updates in
    /// place; only SCHEMA/model changes require a new epoch.
    pub fn upsert_note(
        &mut self,
        note: &Note,
        embedder: &dyn Embedder,
        params: ChunkParams,
    ) -> Result<(), IndexError> {
        let chunks = chunk_text(&note.body, params);
        let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
        let vectors = embedder.embed(&texts)?;
        self.upsert_note_prechunked(note, embedder, &chunks, &vectors)
    }

    /// [`Self::upsert_note`] for callers that chunk and embed OUTSIDE the
    /// index — the batch-ingest path, where every chunk of a whole ingest
    /// run goes through one `Embedder::embed` call. `vectors[i]` embeds
    /// `chunks[i]`; the embedder is still required so the model-identity
    /// guard holds on this path too.
    pub fn upsert_note_prechunked(
        &mut self,
        note: &Note,
        embedder: &dyn Embedder,
        chunks: &[Chunk],
        vectors: &[Vec<f32>],
    ) -> Result<(), IndexError> {
        // A different model must never write vectors into this file, no
        // matter what the caller opened it with.
        check_meta(&self.meta, embedder)?;
        if chunks.len() != vectors.len() {
            return Err(IndexError::ChunkVectorMismatch {
                chunks: chunks.len(),
                vectors: vectors.len(),
            });
        }

        let kp_id = note.kp_id().to_string();
        let title = note.title();
        // The stored change token covers the WHOLE note (frontmatter +
        // body), never the producer-declared frontmatter checksum: a
        // declared checksum covers only what the producer stamps (e.g. a
        // managed region), and keying change detection on it would make
        // user edits outside that region invisible to re-indexing.
        let checksum = Some(note.change_token().to_string());
        let (tags_json, source, created, updated) = match &note.frontmatter {
            Frontmatter::Kp(fm) => (
                serde_json::to_string(&fm.tags).expect("string vec serializes"),
                fm.source.clone(),
                fm.created.clone(),
                fm.updated.clone(),
            ),
            Frontmatter::None | Frontmatter::Foreign(_) => ("[]".to_owned(), None, None, None),
        };

        let tx = self.conn.transaction()?;
        // A note re-minted at the same path is a REPLACEMENT: evict any
        // row holding this path under a different identity, or the path
        // UNIQUE constraint would refuse the upsert.
        delete_notes_where(
            &tx,
            "path = ?1 AND kp_id <> ?2",
            params![note.rel_path, kp_id],
        )?;
        // Clear this note's own chunk state before re-inserting.
        tx.execute(
            "DELETE FROM vec_chunks WHERE rowid IN (SELECT id FROM chunks WHERE note = ?1)",
            params![kp_id],
        )?;
        tx.execute("DELETE FROM chunks WHERE note = ?1", params![kp_id])?;
        tx.execute(
            "INSERT INTO notes (kp_id, path, title, tags, source, created, updated, checksum,
                                body, ingested_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
             ON CONFLICT(kp_id) DO UPDATE SET
               path = excluded.path, title = excluded.title, tags = excluded.tags,
               source = excluded.source, created = excluded.created,
               updated = excluded.updated, checksum = excluded.checksum,
               body = excluded.body, ingested_at = excluded.ingested_at",
            params![
                kp_id,
                note.rel_path,
                title,
                tags_json,
                source,
                created,
                updated,
                checksum,
                note.body
            ],
        )?;
        for (chunk, vector) in chunks.iter().zip(vectors) {
            if vector.len() != self.meta.dims {
                return Err(IndexError::WrongDims {
                    id: embedder.id().to_owned(),
                    got: vector.len(),
                    expected: self.meta.dims,
                });
            }
            tx.execute(
                "INSERT INTO chunks (note, ord, text, token_len) VALUES (?1, ?2, ?3, ?4)",
                params![kp_id, chunk.ord as i64, chunk.text, chunk.token_len as i64],
            )?;
            let rowid = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO vec_chunks (rowid, embedding) VALUES (?1, ?2)",
                params![rowid, vec_to_blob(vector)],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Remove a note and all its derived rows.
    pub fn remove_note(&mut self, kp_id: &str) -> Result<bool, IndexError> {
        let tx = self.conn.transaction()?;
        let removed = delete_notes_where(&tx, "kp_id = ?1", params![kp_id])?;
        tx.commit()?;
        Ok(removed > 0)
    }

    /// Number of indexed notes.
    pub fn note_count(&self) -> Result<i64, IndexError> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0))?)
    }

    /// Record a typed edge (idempotent).
    pub fn add_link(&mut self, from_id: &str, to_id: &str, kind: &str) -> Result<(), IndexError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO links (from_id, to_id, kind) VALUES (?1, ?2, ?3)",
            params![from_id, to_id, kind],
        )?;
        Ok(())
    }

    /// Advance a consumer's tail cursor over an events file (rotation-aware:
    /// one row per (consumer, file)).
    pub fn set_cursor(&mut self, consumer: &str, file: &str, line: i64) -> Result<(), IndexError> {
        self.conn.execute(
            "INSERT INTO cursors (consumer, file, line) VALUES (?1, ?2, ?3)
             ON CONFLICT(consumer, file) DO UPDATE SET line = excluded.line",
            params![consumer, file, line],
        )?;
        Ok(())
    }

    /// Read back a consumer's cursor for one file.
    pub fn cursor(&self, consumer: &str, file: &str) -> Result<Option<i64>, IndexError> {
        Ok(self
            .conn
            .query_row(
                "SELECT line FROM cursors WHERE consumer = ?1 AND file = ?2",
                params![consumer, file],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?)
    }

    /// Every `(file, line)` cursor a consumer holds, sorted by file name.
    pub fn cursors_for(&self, consumer: &str) -> Result<Vec<(String, i64)>, IndexError> {
        let mut stmt = self
            .conn
            .prepare("SELECT file, line FROM cursors WHERE consumer = ?1 ORDER BY file")?;
        let rows = stmt
            .query_map(params![consumer], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Drop a consumer's cursor for one file (housekeeping when the
    /// producer's retention deletes the file).
    pub fn remove_cursor(&mut self, consumer: &str, file: &str) -> Result<(), IndexError> {
        self.conn.execute(
            "DELETE FROM cursors WHERE consumer = ?1 AND file = ?2",
            params![consumer, file],
        )?;
        Ok(())
    }

    /// Record an event id as folded. Returns `true` when the id is NEW
    /// (the caller should fold it) and `false` on a replay (skip).
    ///
    /// Prefer [`Self::fold_event`] when a behavioral delta accompanies
    /// the event — it makes the seen-mark and the fold one transaction.
    pub fn mark_event_seen(
        &mut self,
        consumer: &str,
        event_id: &str,
        ts: &str,
    ) -> Result<bool, IndexError> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO seen_events (consumer, event_id, ts) VALUES (?1, ?2, ?3)",
            params![consumer, event_id, ts],
        )?;
        Ok(n > 0)
    }

    /// Atomically mark an event seen AND fold its behavioral delta — ONE
    /// transaction. Returns `true` when the event was new (and any delta
    /// was folded), `false` on a replay (nothing folded).
    ///
    /// The single transaction is the point: as two separate autocommit
    /// statements, a crash (or busy error) between the seen-mark and the
    /// fold would leave the event marked seen but never folded — and
    /// every retry would then skip it as a duplicate, permanently.
    pub fn fold_event(
        &mut self,
        consumer: &str,
        event_id: &str,
        ts: &str,
        delta: Option<(&str, &BehaviorDelta)>,
    ) -> Result<bool, IndexError> {
        let tx = self.conn.transaction()?;
        let newly = tx.execute(
            "INSERT OR IGNORE INTO seen_events (consumer, event_id, ts) VALUES (?1, ?2, ?3)",
            params![consumer, event_id, ts],
        )? > 0;
        if newly && let Some((kp_id, delta)) = delta {
            apply_behavior_conn(&tx, kp_id, delta)?;
        }
        tx.commit()?;
        Ok(newly)
    }

    /// Prune seen-event ids with `ts` strictly before `before_ts` (RFC 3339
    /// UTC strings compare lexicographically). Returns the rows removed.
    pub fn prune_seen_events(
        &mut self,
        consumer: &str,
        before_ts: &str,
    ) -> Result<usize, IndexError> {
        Ok(self.conn.execute(
            "DELETE FROM seen_events WHERE consumer = ?1 AND ts < ?2",
            params![consumer, before_ts],
        )?)
    }

    /// Fold one behavioral delta into a note's rollup row. `starred` /
    /// `read_later` of `None` leave the flag untouched; `activity_ts`
    /// advances `last_activity` monotonically (max of old and new).
    pub fn apply_behavior(&mut self, kp_id: &str, delta: &BehaviorDelta) -> Result<(), IndexError> {
        apply_behavior_conn(&self.conn, kp_id, delta)
    }

    /// A note's behavioral rollup, if any events have folded into it.
    pub fn behavior(&self, kp_id: &str) -> Result<Option<BehaviorStats>, IndexError> {
        behavior_row(&self.conn, kp_id)
    }

    /// Record a librarian digest (idempotent by date — the `digest_log`
    /// UNIQUE constraint). Returns `true` when the row is new, `false`
    /// when this date was already logged.
    pub fn record_digest(
        &mut self,
        digest_date: &str,
        kp_id: &str,
        created: &str,
    ) -> Result<bool, IndexError> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO digest_log (digest_date, kp_id, created) VALUES (?1, ?2, ?3)",
            params![digest_date, kp_id, created],
        )?;
        Ok(n > 0)
    }

    /// The indexed `(path, checksum)` of a note, if present — the change
    /// detector: an unchanged (checksum, path) pair skips re-embedding.
    pub fn note_state(&self, kp_id: &str) -> Result<Option<NoteState>, IndexError> {
        let row = self.conn.query_row(
            "SELECT path, checksum FROM notes WHERE kp_id = ?1",
            params![kp_id],
            |r| {
                Ok(NoteState {
                    path: r.get(0)?,
                    checksum: r.get(1)?,
                })
            },
        );
        match row {
            Ok(state) => Ok(Some(state)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Every indexed `(kp_id, path)` pair — lets ingest prune notes whose
    /// files vanished from the vault.
    pub fn note_ids_and_paths(&self) -> Result<Vec<(String, String)>, IndexError> {
        let mut stmt = self.conn.prepare("SELECT kp_id, path FROM notes")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Drop every outgoing edge of a note (before re-adding from a fresh
    /// parse — link rows must not accrete across re-ingests).
    pub fn clear_links_from(&mut self, from_id: &str) -> Result<(), IndexError> {
        self.conn
            .execute("DELETE FROM links WHERE from_id = ?1", params![from_id])?;
        Ok(())
    }

    /// A note's outgoing edges as `(to_id, kind)`, ordered.
    pub fn links_from(&self, from_id: &str) -> Result<Vec<(String, String)>, IndexError> {
        let mut stmt = self
            .conn
            .prepare("SELECT to_id, kind FROM links WHERE from_id = ?1 ORDER BY to_id, kind")?;
        let rows = stmt
            .query_map(params![from_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// `PRAGMA integrity_check` — used by the epoch machinery before a
    /// blue/green swap.
    pub fn integrity_check(&self) -> Result<(), IndexError> {
        let verdict: String = self
            .conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if verdict == "ok" {
            Ok(())
        } else {
            Err(IndexError::EpochVerification(verdict))
        }
    }

    /// Close the writer, surfacing any error (drop would swallow it).
    /// A clean close checkpoints and removes the WAL sidecars — required
    /// before an epoch swap renames files around.
    pub fn close(self) -> Result<(), IndexError> {
        self.conn.close().map_err(|(_, e)| IndexError::Sqlite(e))
    }
}

impl IndexReader {
    /// Open a read-only connection to an index file.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, IndexError> {
        register_sqlite_vec();
        let path = path.as_ref().to_owned();
        if !path.exists() {
            return Err(IndexError::Missing(path));
        }
        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let meta = read_meta(&conn, &path)?;
        Ok(Self { conn, meta })
    }

    /// The meta row (schema/embedder/epoch) of the open file.
    #[must_use]
    pub fn meta(&self) -> &IndexMeta {
        &self.meta
    }

    /// Guard used by vector legs: the query embedder must match the index.
    pub(crate) fn check_embedder(&self, embedder: &dyn Embedder) -> Result<(), IndexError> {
        check_meta(&self.meta, embedder)
    }

    /// A note's behavioral rollup — the reader twin of [`Index::behavior`]
    /// (the librarian's scoring reads these without a writer handle).
    pub fn behavior(&self, kp_id: &str) -> Result<Option<BehaviorStats>, IndexError> {
        behavior_row(&self.conn, kp_id)
    }

    /// Every `(file, line)` cursor a consumer holds — the reader twin of
    /// [`Index::cursors_for`] (`kp doctor` inspects tail health without a
    /// writer handle).
    pub fn cursors_for(&self, consumer: &str) -> Result<Vec<(String, i64)>, IndexError> {
        let mut stmt = self
            .conn
            .prepare("SELECT file, line FROM cursors WHERE consumer = ?1 ORDER BY file")?;
        let rows = stmt
            .query_map(params![consumer], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Number of indexed notes.
    pub fn note_count(&self) -> Result<i64, IndexError> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0))?)
    }
}

/// The behavior-fold statement, over any connection/transaction (see
/// [`Index::apply_behavior`] for the semantics).
fn apply_behavior_conn(
    conn: &Connection,
    kp_id: &str,
    delta: &BehaviorDelta,
) -> Result<(), IndexError> {
    conn.execute(
        "INSERT INTO behavior (kp_id, opened_count, starred, read_later, last_activity)
         VALUES (?1, ?2, COALESCE(?3, 0), COALESCE(?4, 0), ?5)
         ON CONFLICT(kp_id) DO UPDATE SET
           opened_count = opened_count + ?2,
           starred      = COALESCE(?3, starred),
           read_later   = COALESCE(?4, read_later),
           last_activity = CASE
             WHEN ?5 IS NULL THEN last_activity
             WHEN last_activity IS NULL OR last_activity < ?5 THEN ?5
             ELSE last_activity
           END",
        params![
            kp_id,
            delta.opened_delta,
            delta.starred.map(i64::from),
            delta.read_later.map(i64::from),
            delta.activity_ts,
        ],
    )?;
    Ok(())
}

/// Shared behavior-rollup lookup (writer and reader connections).
fn behavior_row(conn: &Connection, kp_id: &str) -> Result<Option<BehaviorStats>, IndexError> {
    let row = conn.query_row(
        "SELECT opened_count, starred, read_later, last_activity
         FROM behavior WHERE kp_id = ?1",
        params![kp_id],
        |r| {
            Ok(BehaviorStats {
                opened_count: r.get(0)?,
                starred: r.get::<_, i64>(1)? != 0,
                read_later: r.get::<_, i64>(2)? != 0,
                last_activity: r.get(3)?,
            })
        },
    );
    match row {
        Ok(stats) => Ok(Some(stats)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn read_meta(conn: &Connection, path: &Path) -> Result<IndexMeta, IndexError> {
    let row = conn.query_row(
        "SELECT schema_version, embedder_model, dims, epoch, built_at FROM meta WHERE id = 1",
        [],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
            ))
        },
    );
    let (schema_version, embedder_id, dims, epoch, built_at) = match row {
        Ok(row) => row,
        Err(_) => return Err(IndexError::CorruptMeta(path.to_owned())),
    };
    if schema_version != SCHEMA_VERSION {
        return Err(IndexError::SchemaVersion {
            found: schema_version,
            supported: SCHEMA_VERSION,
        });
    }
    let dims = usize::try_from(dims).map_err(|_| IndexError::CorruptMeta(path.to_owned()))?;
    Ok(IndexMeta {
        schema_version,
        embedder_id,
        dims,
        epoch,
        built_at,
    })
}

fn check_meta(meta: &IndexMeta, embedder: &dyn Embedder) -> Result<(), IndexError> {
    if meta.embedder_id != embedder.id() || meta.dims != embedder.dims() {
        return Err(IndexError::EmbedderMismatch {
            index_id: meta.embedder_id.clone(),
            index_dims: meta.dims,
            embedder_id: embedder.id().to_owned(),
            embedder_dims: embedder.dims(),
        });
    }
    Ok(())
}

/// Delete notes matching a WHERE clause plus every derived row (chunks,
/// vectors; FTS rides the delete trigger).
fn delete_notes_where(
    conn: &Connection,
    where_clause: &str,
    args: impl rusqlite::Params + Copy,
) -> Result<usize, IndexError> {
    conn.execute(
        &format!(
            "DELETE FROM vec_chunks WHERE rowid IN
               (SELECT c.id FROM chunks c JOIN notes n ON n.kp_id = c.note WHERE {where_clause})"
        ),
        args,
    )?;
    conn.execute(
        &format!("DELETE FROM chunks WHERE note IN (SELECT kp_id FROM notes WHERE {where_clause})"),
        args,
    )?;
    let n = conn.execute(&format!("DELETE FROM notes WHERE {where_clause}"), args)?;
    Ok(n)
}

/// Serialize an f32 vector into sqlite-vec's little-endian float32 blob.
fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

fn create_schema(conn: &Connection, dims: usize) -> Result<(), IndexError> {
    // notes.body exists because notes_fts is an EXTERNAL-CONTENT FTS5
    // table over (title, body). The alternatives both lose:
    //   - contentless-delete ('content=', contentless_delete=1) stores no
    //     text at all, so snippet()/highlight() cannot reconstruct match
    //     context — and snippets are part of the search contract surface
    //     (kp_search returns them);
    //   - a plain (self-contained) FTS5 table stores its own full copy of
    //     title+body IN ADDITION to wherever we keep the body for
    //     kp_get_note, i.e. the text twice.
    // External content stores the text exactly once (in notes, where
    // kp_get_note wants it anyway) while keeping snippet() working; the
    // triggers below are the standard FTS5 external-content contract and
    // keep the index in lockstep with zero caller discipline.
    conn.execute_batch(&format!(
        "
        CREATE TABLE meta (
          id             INTEGER PRIMARY KEY CHECK (id = 1),
          schema_version INTEGER NOT NULL,
          embedder_model TEXT    NOT NULL,
          dims           INTEGER NOT NULL,
          epoch          INTEGER NOT NULL,
          built_at       TEXT    NOT NULL
        );

        CREATE TABLE notes (
          kp_id       TEXT PRIMARY KEY,
          path        TEXT NOT NULL UNIQUE,
          title       TEXT NOT NULL,
          tags        TEXT NOT NULL DEFAULT '[]',  -- JSON array
          source      TEXT,
          created     TEXT,
          updated     TEXT,
          checksum    TEXT,                        -- change token, never identity
          body        TEXT NOT NULL,               -- FTS external content + kp_get_note
          ingested_at TEXT NOT NULL
        );

        CREATE TABLE chunks (
          id        INTEGER PRIMARY KEY,
          note      TEXT    NOT NULL REFERENCES notes(kp_id),
          ord       INTEGER NOT NULL,
          text      TEXT    NOT NULL,
          token_len INTEGER NOT NULL,
          UNIQUE (note, ord)
        );
        CREATE INDEX chunks_note ON chunks(note);

        -- Chunk vectors; rowid = chunks.id. Cosine distance to match the
        -- retrieval contract (RRF over cosine + BM25).
        CREATE VIRTUAL TABLE vec_chunks USING vec0(
          embedding float[{dims}] distance_metric=cosine
        );

        CREATE VIRTUAL TABLE notes_fts USING fts5(
          title, body,
          content='notes', content_rowid='rowid'
        );
        CREATE TRIGGER notes_fts_ai AFTER INSERT ON notes BEGIN
          INSERT INTO notes_fts(rowid, title, body)
            VALUES (new.rowid, new.title, new.body);
        END;
        CREATE TRIGGER notes_fts_ad AFTER DELETE ON notes BEGIN
          INSERT INTO notes_fts(notes_fts, rowid, title, body)
            VALUES ('delete', old.rowid, old.title, old.body);
        END;
        CREATE TRIGGER notes_fts_au AFTER UPDATE ON notes BEGIN
          INSERT INTO notes_fts(notes_fts, rowid, title, body)
            VALUES ('delete', old.rowid, old.title, old.body);
          INSERT INTO notes_fts(rowid, title, body)
            VALUES (new.rowid, new.title, new.body);
        END;

        -- The edge graph: plain relational rows, recursive CTEs for
        -- expansion. No graph database, by decision (decisions.md §1).
        CREATE TABLE links (
          from_id TEXT NOT NULL,
          to_id   TEXT NOT NULL,
          kind    TEXT NOT NULL,
          PRIMARY KEY (from_id, to_id, kind)
        );
        CREATE INDEX links_to ON links(to_id);

        -- Tail cursors for the Curio events JSONL (rotation-aware: the
        -- consumer tracks (file, line) per physical file).
        CREATE TABLE cursors (
          consumer TEXT NOT NULL,
          file     TEXT NOT NULL,
          line     INTEGER NOT NULL,
          PRIMARY KEY (consumer, file)
        );

        -- Behavioral rollups (from Curio events). Derived and disposable,
        -- like everything here — raw behavioral history is NEVER committed
        -- to git, and only these aggregates land in the index.
        CREATE TABLE behavior (
          kp_id         TEXT PRIMARY KEY,
          opened_count  INTEGER NOT NULL DEFAULT 0,
          starred       INTEGER NOT NULL DEFAULT 0,
          read_later    INTEGER NOT NULL DEFAULT 0,
          last_activity TEXT
        );

        -- Event ids already folded into `behavior`, per consumer. Makes
        -- replay idempotent when a cursor's file vanishes and the tail
        -- restarts from the oldest existing file. Pruned by ts: ids older
        -- than the oldest existing event file can never be replayed.
        CREATE TABLE seen_events (
          consumer TEXT NOT NULL,
          event_id TEXT NOT NULL,
          ts       TEXT NOT NULL,
          PRIMARY KEY (consumer, event_id)
        );
        CREATE INDEX seen_events_ts ON seen_events(consumer, ts);

        -- One row per librarian digest (digests are idempotent by date).
        CREATE TABLE digest_log (
          id          INTEGER PRIMARY KEY,
          digest_date TEXT NOT NULL UNIQUE,
          kp_id       TEXT NOT NULL,
          created     TEXT NOT NULL
        );
        "
    ))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::HashEmbedder;

    fn tmp_index(dir: &Path) -> (Index, HashEmbedder) {
        let e = HashEmbedder::new(32);
        let idx = Index::create(dir.join("index.db"), &e, 1).expect("create");
        (idx, e)
    }

    fn note(rel: &str, body: &str) -> Note {
        Note::parse(rel, body).expect("parses")
    }

    #[test]
    fn create_then_open_round_trips_meta() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (idx, e) = tmp_index(dir.path());
        let path = idx.path().to_owned();
        assert_eq!(idx.meta().embedder_id, "hash");
        assert_eq!(idx.meta().dims, 32);
        assert_eq!(idx.meta().epoch, 1);
        idx.close().expect("close");
        let idx = Index::open(&path, &e).expect("reopen");
        assert_eq!(idx.meta().schema_version, SCHEMA_VERSION);
        assert_eq!(idx.meta().epoch, 1);
    }

    #[test]
    fn open_missing_index_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let e = HashEmbedder::new(32);
        assert!(matches!(
            Index::open(dir.path().join("nope.db"), &e).unwrap_err(),
            IndexError::Missing(_)
        ));
    }

    #[test]
    fn mixed_model_indexes_are_forbidden() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (idx, _) = tmp_index(dir.path());
        let path = idx.path().to_owned();
        idx.close().expect("close");

        // Same id, different dims.
        let err = Index::open(&path, &HashEmbedder::new(64)).unwrap_err();
        assert!(
            matches!(err, IndexError::EmbedderMismatch { .. }),
            "got {err}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("rebuild"),
            "error must demand a rebuild: {msg}"
        );

        // Different id entirely.
        struct OtherEmbedder;
        impl Embedder for OtherEmbedder {
            fn id(&self) -> &str {
                "other-model-v9"
            }
            fn dims(&self) -> usize {
                32
            }
            fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, crate::embed::EmbedError> {
                Ok(texts.iter().map(|_| vec![0.0; 32]).collect())
            }
        }
        let err = Index::open(&path, &OtherEmbedder).unwrap_err();
        assert!(matches!(
            err,
            IndexError::EmbedderMismatch { ref index_id, ref embedder_id, .. }
                if index_id == "hash" && embedder_id == "other-model-v9"
        ));

        // And the write path enforces it independently of open().
        let mut idx = Index::open(&path, &HashEmbedder::new(32)).expect("open");
        let err = idx
            .upsert_note(
                &note("a.md", "text"),
                &OtherEmbedder,
                ChunkParams::default(),
            )
            .unwrap_err();
        assert!(matches!(err, IndexError::EmbedderMismatch { .. }));
    }

    #[test]
    fn upsert_is_incremental_and_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut idx, e) = tmp_index(dir.path());
        let p = ChunkParams {
            tokens: 4,
            overlap: 1,
        };

        idx.upsert_note(&note("a.md", "alpha beta gamma delta epsilon zeta"), &e, p)
            .expect("insert");
        assert_eq!(idx.note_count().expect("count"), 1);
        let chunk_rows: i64 = idx
            .conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
            .expect("count");
        let vec_rows: i64 = idx
            .conn
            .query_row("SELECT COUNT(*) FROM vec_chunks", [], |r| r.get(0))
            .expect("count");
        assert_eq!(chunk_rows, vec_rows, "every chunk has exactly one vector");
        assert!(chunk_rows >= 2);

        // Re-ingesting a changed body replaces rows instead of accreting.
        idx.upsert_note(&note("a.md", "short now"), &e, p)
            .expect("update");
        assert_eq!(idx.note_count().expect("count"), 1);
        let chunk_rows: i64 = idx
            .conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
            .expect("count");
        assert_eq!(chunk_rows, 1);
        let body: String = idx
            .conn
            .query_row(
                "SELECT body FROM notes WHERE kp_id = 'path:a.md'",
                [],
                |r| r.get(0),
            )
            .expect("body");
        assert_eq!(body, "short now");
    }

    #[test]
    fn reminted_identity_at_same_path_replaces_the_old_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut idx, e) = tmp_index(dir.path());
        let p = ChunkParams::default();
        idx.upsert_note(&note("a.md", "plain body"), &e, p)
            .expect("insert");
        // Same path, now with minted identity.
        let minted = note(
            "a.md",
            "---\nkp_id: \"kp:0197\"\nkp_schema: kp-note/v1\ntitle: Minted\n---\nnew body\n",
        );
        idx.upsert_note(&minted, &e, p).expect("replace");
        assert_eq!(idx.note_count().expect("count"), 1);
        let id: String = idx
            .conn
            .query_row("SELECT kp_id FROM notes WHERE path = 'a.md'", [], |r| {
                r.get(0)
            })
            .expect("id");
        assert_eq!(id, "kp:0197");
    }

    #[test]
    fn remove_note_clears_all_derived_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut idx, e) = tmp_index(dir.path());
        idx.upsert_note(&note("a.md", "some body text"), &e, ChunkParams::default())
            .expect("insert");
        assert!(idx.remove_note("path:a.md").expect("remove"));
        assert!(
            !idx.remove_note("path:a.md")
                .expect("second remove is a no-op")
        );
        for table in ["notes", "chunks", "vec_chunks"] {
            let n: i64 = idx
                .conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .expect("count");
            assert_eq!(n, 0, "{table} not empty");
        }
        // FTS returns nothing after the delete trigger ran.
        let n: i64 = idx
            .conn
            .query_row(
                "SELECT COUNT(*) FROM notes_fts WHERE notes_fts MATCH 'body'",
                [],
                |r| r.get(0),
            )
            .expect("fts count");
        assert_eq!(n, 0);
    }

    #[test]
    fn frontmatter_metadata_lands_in_columns() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut idx, e) = tmp_index(dir.path());
        let n = note(
            "curio/x.md",
            "---\nkp_id: \"curio:abc\"\nkp_schema: kp-note/v1\ntitle: \"T\"\ntags: [rust, db]\nsource: \"https://example.com/a\"\ncreated: 2026-07-01T00:00:00Z\n---\nbody words\n",
        );
        idx.upsert_note(&n, &e, ChunkParams::default())
            .expect("insert");
        let (title, tags, source, checksum): (String, String, String, String) = idx
            .conn
            .query_row(
                "SELECT title, tags, source, checksum FROM notes WHERE kp_id = 'curio:abc'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .expect("row");
        assert_eq!(title, "T");
        assert_eq!(tags, "[\"rust\",\"db\"]");
        assert_eq!(source, "https://example.com/a");
        assert!(checksum.starts_with("sha256:"));
    }

    #[test]
    fn cursors_and_links_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut idx, _) = tmp_index(dir.path());
        assert_eq!(
            idx.cursor("curio-events", "2026-07.jsonl").expect("get"),
            None
        );
        idx.set_cursor("curio-events", "2026-07.jsonl", 42)
            .expect("set");
        idx.set_cursor("curio-events", "2026-07.jsonl", 99)
            .expect("advance");
        assert_eq!(
            idx.cursor("curio-events", "2026-07.jsonl").expect("get"),
            Some(99)
        );

        idx.add_link("kp:a", "kp:b", "references").expect("link");
        idx.add_link("kp:a", "kp:b", "references")
            .expect("idempotent");
        let n: i64 = idx
            .conn
            .query_row("SELECT COUNT(*) FROM links", [], |r| r.get(0))
            .expect("count");
        assert_eq!(n, 1);
    }

    #[test]
    fn prechunked_upsert_matches_the_inline_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut idx, e) = tmp_index(dir.path());
        let n = note("a.md", "alpha beta gamma");
        let chunks = chunk_text(&n.body, ChunkParams::default());
        let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
        let vectors = e.embed(&texts).expect("embeds");
        idx.upsert_note_prechunked(&n, &e, &chunks, &vectors)
            .expect("upsert");
        assert_eq!(idx.note_count().expect("count"), 1);
        // Length mismatch is refused before any write.
        let err = idx
            .upsert_note_prechunked(&n, &e, &chunks, &[])
            .unwrap_err();
        assert!(matches!(err, IndexError::ChunkVectorMismatch { .. }));
    }

    #[test]
    fn seen_events_dedupe_and_prune() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut idx, _) = tmp_index(dir.path());
        assert!(
            idx.mark_event_seen("c", "01AAA", "2026-07-01T00:00:00.000Z")
                .expect("mark")
        );
        assert!(
            !idx.mark_event_seen("c", "01AAA", "2026-07-01T00:00:00.000Z")
                .expect("replay"),
            "second sighting must report already-seen"
        );
        // Another consumer's view is independent.
        assert!(
            idx.mark_event_seen("other", "01AAA", "2026-07-01T00:00:00.000Z")
                .expect("mark")
        );
        assert!(
            idx.mark_event_seen("c", "01BBB", "2026-07-02T00:00:00.000Z")
                .expect("mark")
        );
        // Prune strictly-before: 01AAA goes, 01BBB stays.
        let n = idx
            .prune_seen_events("c", "2026-07-02T00:00:00.000Z")
            .expect("prune");
        assert_eq!(n, 1);
        assert!(
            idx.mark_event_seen("c", "01AAA", "2026-07-01T00:00:00.000Z")
                .expect("pruned id is fresh again")
        );
        assert!(
            !idx.mark_event_seen("c", "01BBB", "2026-07-02T00:00:00.000Z")
                .expect("kept id still dedupes")
        );
    }

    #[test]
    fn fold_event_is_one_seen_mark_plus_fold_and_replays_fold_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut idx, _) = tmp_index(dir.path());
        let delta = BehaviorDelta {
            opened_delta: 1,
            activity_ts: Some("2026-07-01T10:00:00.000Z".into()),
            ..Default::default()
        };
        assert!(
            idx.fold_event(
                "c",
                "01AAA",
                "2026-07-01T10:00:00.000Z",
                Some(("kp:x", &delta))
            )
            .expect("first sighting folds")
        );
        // The replay neither folds nor increments — and the seen row and
        // the fold committed TOGETHER (one transaction), so there is no
        // seen-but-unfolded state to get stuck in.
        assert!(
            !idx.fold_event(
                "c",
                "01AAA",
                "2026-07-01T10:00:00.000Z",
                Some(("kp:x", &delta))
            )
            .expect("replay is skipped")
        );
        let stats = idx.behavior("kp:x").expect("query").expect("row");
        assert_eq!(stats.opened_count, 1, "the replay must not double-fold");
        // Non-behavioral events (no delta) still dedupe.
        assert!(
            idx.fold_event("c", "01BBB", "2026-07-01T11:00:00.000Z", None)
                .expect("mark only")
        );
        assert!(
            !idx.fold_event("c", "01BBB", "2026-07-01T11:00:00.000Z", None)
                .expect("replay")
        );
    }

    #[test]
    fn behavior_folds_deltas_and_honors_negation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut idx, _) = tmp_index(dir.path());
        assert_eq!(idx.behavior("curio:x").expect("none"), None);

        idx.apply_behavior(
            "curio:x",
            &BehaviorDelta {
                opened_delta: 1,
                starred: Some(true),
                activity_ts: Some("2026-07-01T10:00:00.000Z".into()),
                ..Default::default()
            },
        )
        .expect("fold");
        idx.apply_behavior(
            "curio:x",
            &BehaviorDelta {
                opened_delta: 1,
                read_later: Some(true),
                activity_ts: Some("2026-07-02T10:00:00.000Z".into()),
                ..Default::default()
            },
        )
        .expect("fold");
        // Negation: unstar. An OLDER ts must not roll last_activity back.
        idx.apply_behavior(
            "curio:x",
            &BehaviorDelta {
                starred: Some(false),
                activity_ts: Some("2026-07-01T12:00:00.000Z".into()),
                ..Default::default()
            },
        )
        .expect("fold");

        let stats = idx.behavior("curio:x").expect("row").expect("present");
        assert_eq!(
            stats,
            BehaviorStats {
                opened_count: 2,
                starred: false,
                read_later: true,
                last_activity: Some("2026-07-02T10:00:00.000Z".into()),
            }
        );
    }

    #[test]
    fn note_state_reports_path_and_checksum() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut idx, e) = tmp_index(dir.path());
        assert_eq!(idx.note_state("path:a.md").expect("none"), None);
        idx.upsert_note(&note("a.md", "body"), &e, ChunkParams::default())
            .expect("insert");
        let state = idx.note_state("path:a.md").expect("row").expect("present");
        assert_eq!(state.path, "a.md");
        assert!(state.checksum.expect("has token").starts_with("sha256:"));
        assert_eq!(
            idx.note_ids_and_paths().expect("list"),
            vec![("path:a.md".to_owned(), "a.md".to_owned())]
        );
    }

    #[test]
    fn cursors_enumerate_and_remove() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut idx, _) = tmp_index(dir.path());
        idx.set_cursor("c", "events-20260702.jsonl", 3)
            .expect("set");
        idx.set_cursor("c", "events-20260701.jsonl", 9)
            .expect("set");
        idx.set_cursor("other", "x.jsonl", 1).expect("set");
        assert_eq!(
            idx.cursors_for("c").expect("list"),
            vec![
                ("events-20260701.jsonl".to_owned(), 9),
                ("events-20260702.jsonl".to_owned(), 3),
            ]
        );
        idx.remove_cursor("c", "events-20260701.jsonl")
            .expect("remove");
        assert_eq!(
            idx.cursors_for("c").expect("list"),
            vec![("events-20260702.jsonl".to_owned(), 3)]
        );
    }

    #[test]
    fn clear_links_from_drops_only_outgoing_edges() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut idx, _) = tmp_index(dir.path());
        idx.add_link("kp:a", "kp:b", "wikilink").expect("link");
        idx.add_link("kp:a", "kp:c", "markdown").expect("link");
        idx.add_link("kp:b", "kp:a", "wikilink").expect("link");
        idx.clear_links_from("kp:a").expect("clear");
        let n: i64 = idx
            .conn
            .query_row("SELECT COUNT(*) FROM links", [], |r| r.get(0))
            .expect("count");
        assert_eq!(n, 1, "incoming edge to kp:a must survive");
    }

    #[test]
    fn readers_are_separate_connections() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut idx, e) = tmp_index(dir.path());
        idx.upsert_note(&note("a.md", "hello reader"), &e, ChunkParams::default())
            .expect("insert");
        idx.set_cursor("c", "events-20260701.jsonl", 7)
            .expect("set");
        let reader = idx.reader().expect("reader");
        assert_eq!(reader.meta(), idx.meta());
        // Reader twins mirror the writer's view.
        assert_eq!(reader.note_count().expect("count"), 1);
        assert_eq!(
            reader.cursors_for("c").expect("list"),
            vec![("events-20260701.jsonl".to_owned(), 7)]
        );
        // Read-only connection really is read-only.
        assert!(reader.conn.execute("DELETE FROM notes", []).is_err());
        // And WAL lets it read while the writer holds changes.
        let n: i64 = reader
            .conn
            .query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0))
            .expect("count");
        assert_eq!(n, 1);
    }

    #[test]
    fn integrity_check_passes_on_fresh_index() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (idx, _) = tmp_index(dir.path());
        idx.integrity_check().expect("fresh index is sound");
    }
}
