//! Read-side query surface for serving contexts (MCP tools, CLI reads).
//!
//! Everything here lives on [`IndexReader`] — a read-only connection —
//! and serves straight from the index file: note lookup by identity,
//! embedding-nearest neighbors from STORED chunk vectors (no query-time
//! embedding), recency listings, and the latest librarian digest. Like
//! the rest of this crate it is internal, not a published contract; the
//! published shapes live in `contracts/mcp/v1.md` and are rendered by
//! kp-mcp on top of these types.

use rusqlite::params;

use crate::db::IndexReader;
use crate::error::IndexError;
use crate::search::SearchHit;

/// A note as the index knows it: identity + metadata columns + body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteRecord {
    pub kp_id: String,
    /// Vault-relative path as indexed.
    pub path: String,
    pub title: String,
    pub tags: Vec<String>,
    pub source: Option<String>,
    /// Frontmatter-declared creation timestamp, when present.
    pub created: Option<String>,
    /// Frontmatter-declared update timestamp, when present.
    pub updated: Option<String>,
    /// The change token (`sha256:<hex>`), when recorded.
    pub checksum: Option<String>,
    /// The full markdown body as indexed.
    pub body: String,
    /// When the index last wrote this note (RFC 3339 UTC).
    pub ingested_at: String,
}

/// A note listing row (no body) — what recency queries return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteSummary {
    pub kp_id: String,
    pub path: String,
    pub title: String,
    pub tags: Vec<String>,
    pub source: Option<String>,
    pub updated: Option<String>,
    pub ingested_at: String,
}

const NOTE_COLUMNS: &str =
    "kp_id, path, title, tags, source, created, updated, checksum, body, ingested_at";

fn note_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<NoteRecord> {
    Ok(NoteRecord {
        kp_id: r.get(0)?,
        path: r.get(1)?,
        title: r.get(2)?,
        tags: parse_tags(&r.get::<_, String>(3)?),
        source: r.get(4)?,
        created: r.get(5)?,
        updated: r.get(6)?,
        checksum: r.get(7)?,
        body: r.get(8)?,
        ingested_at: r.get(9)?,
    })
}

/// Tags are stored as a JSON array string; a malformed cell (impossible
/// via the writer) degrades to no tags rather than failing the read.
fn parse_tags(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_default()
}

impl IndexReader {
    /// Fetch one note by exact `kp_id` (any identity namespace).
    pub fn get_note(&self, kp_id: &str) -> Result<Option<NoteRecord>, IndexError> {
        let row = self.conn.query_row(
            &format!("SELECT {NOTE_COLUMNS} FROM notes WHERE kp_id = ?1"),
            params![kp_id],
            note_from_row,
        );
        match row {
            Ok(note) => Ok(Some(note)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// A note's outgoing edges as `(to_id, kind)`, ordered — the reader
    /// twin of the writer's `links_from`.
    pub fn links_from(&self, from_id: &str) -> Result<Vec<(String, String)>, IndexError> {
        let mut stmt = self
            .conn
            .prepare("SELECT to_id, kind FROM links WHERE from_id = ?1 ORDER BY to_id, kind")?;
        let rows = stmt
            .query_map(params![from_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Embedding-nearest notes to `kp_id`, from STORED vectors: the
    /// note's chunk centroid is the query point, its own chunks are
    /// excluded, chunk hits collapse to their best chunk per note. Score
    /// is cosine similarity. Empty when the note is unknown, has no
    /// chunks, or embeds to a zero vector.
    pub fn related(&self, kp_id: &str, k: usize) -> Result<Vec<SearchHit>, IndexError> {
        let Some(centroid) = self.chunk_centroid(kp_id)? else {
            return Ok(Vec::new());
        };
        let mut blob = Vec::with_capacity(centroid.len() * 4);
        for x in &centroid {
            blob.extend_from_slice(&x.to_le_bytes());
        }
        // Over-fetch: own chunks come back too (they are nearest by
        // construction) and several hits may share a note.
        let own_chunks: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM chunks WHERE note = ?1",
            params![kp_id],
            |r| r.get(0),
        )?;
        let k_chunks = (k.saturating_add(1).saturating_mul(3)) as i64 + own_chunks;
        let mut stmt = self.conn.prepare(
            "SELECT n.kp_id, n.title, n.path, c.text, v.distance
             FROM (SELECT rowid, distance FROM vec_chunks
                   WHERE embedding MATCH ?1 AND k = ?2) v
             JOIN chunks c ON c.id = v.rowid
             JOIN notes n ON n.kp_id = c.note
             WHERE c.note <> ?3
             ORDER BY v.distance",
        )?;
        let rows = stmt.query_map(params![blob, k_chunks, kp_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, f64>(4)?,
            ))
        })?;
        let mut hits: Vec<SearchHit> = Vec::new();
        for row in rows {
            let (kp_id, title, path, chunk_text, distance) = row?;
            if hits.len() == k {
                break;
            }
            if hits.iter().any(|h| h.kp_id == kp_id) {
                continue; // best (lowest-distance) chunk already kept
            }
            hits.push(SearchHit {
                kp_id,
                title,
                path,
                snippet: crate::search::clip(&chunk_text, 160),
                score: 1.0 - distance,
            });
        }
        Ok(hits)
    }

    /// Notes the index wrote at or after `cutoff` (RFC 3339 UTC string —
    /// `ingested_at` updates on every upsert, so this is "recently
    /// ingested or changed"). `namespace` filters on the `kp_id` identity
    /// namespace (`curio` | `zotero` | `kp` | `path`). Newest first,
    /// `kp_id` as the deterministic tie-break.
    pub fn recent_since(
        &self,
        cutoff: &str,
        namespace: Option<&str>,
        limit: usize,
    ) -> Result<Vec<NoteSummary>, IndexError> {
        let prefix = namespace.map(|ns| format!("{ns}:"));
        let mut stmt = self.conn.prepare(
            "SELECT kp_id, path, title, tags, source, updated, ingested_at
             FROM notes
             WHERE ingested_at >= ?1
               AND (?2 IS NULL OR substr(kp_id, 1, length(?2)) = ?2)
             ORDER BY ingested_at DESC, kp_id
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![cutoff, prefix, limit as i64], |r| {
            Ok(NoteSummary {
                kp_id: r.get(0)?,
                path: r.get(1)?,
                title: r.get(2)?,
                tags: parse_tags(&r.get::<_, String>(3)?),
                source: r.get(4)?,
                updated: r.get(5)?,
                ingested_at: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// The latest librarian digest note: the `digest_log` row with the
    /// greatest `digest_date` when the librarian has recorded one, else —
    /// digests are date-named and create-only — the lexicographically
    /// last `kp:`-minted note under `digest_dir`.
    pub fn latest_digest(&self, digest_dir: &str) -> Result<Option<NoteRecord>, IndexError> {
        let logged: Option<String> = self
            .conn
            .query_row(
                "SELECT kp_id FROM digest_log ORDER BY digest_date DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        if let Some(kp_id) = logged
            && let Some(note) = self.get_note(&kp_id)?
        {
            return Ok(Some(note));
        }
        let prefix = format!("{}/", digest_dir.trim_end_matches('/'));
        let row = self.conn.query_row(
            &format!(
                "SELECT {NOTE_COLUMNS} FROM notes
                 WHERE substr(path, 1, length(?1)) = ?1
                   AND substr(kp_id, 1, 3) = 'kp:'
                 ORDER BY path DESC LIMIT 1"
            ),
            params![prefix],
            note_from_row,
        );
        match row {
            Ok(note) => Ok(Some(note)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// The mean of a note's stored chunk vectors, or `None` when the note
    /// has no chunks or the centroid has zero norm.
    fn chunk_centroid(&self, kp_id: &str) -> Result<Option<Vec<f32>>, IndexError> {
        let mut stmt = self.conn.prepare(
            "SELECT v.embedding FROM vec_chunks v
             JOIN chunks c ON c.id = v.rowid
             WHERE c.note = ?1",
        )?;
        let blobs = stmt
            .query_map(params![kp_id], |r| r.get::<_, Vec<u8>>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        if blobs.is_empty() {
            return Ok(None);
        }
        let dims = self.meta.dims;
        let mut centroid = vec![0.0f32; dims];
        for blob in &blobs {
            for (i, chunk) in blob.chunks_exact(4).take(dims).enumerate() {
                centroid[i] += f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            }
        }
        let n = blobs.len() as f32;
        for x in &mut centroid {
            *x /= n;
        }
        if centroid.iter().all(|x| *x == 0.0) {
            return Ok(None);
        }
        Ok(Some(centroid))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use kp_core::note::Note;

    use crate::chunk::ChunkParams;
    use crate::db::Index;
    use crate::embed::HashEmbedder;

    fn seeded(dir: &Path) -> (Index, HashEmbedder) {
        let e = HashEmbedder::new(128);
        let mut idx = Index::create(dir.join("index.db"), &e, 1).expect("create");
        let p = ChunkParams {
            tokens: 16,
            overlap: 2,
        };
        // The hash embedder's cosine space is driven by shared tokens, so
        // the two rust notes overlap heavily while the bread note is
        // fully disjoint.
        let notes = [
            (
                "rust/db.md",
                "---\nkp_id: \"kp:aaa\"\nkp_schema: kp-note/v1\ntitle: Rust databases\n\
                 tags: [rust, databases]\nsource: \"https://example.com/rust-db\"\n\
                 updated: 2026-07-01T00:00:00Z\n---\n\
                 rust database embedded sqlite storage engine queries indexes design\n",
            ),
            (
                "rust/async.md",
                "---\nkp_id: \"kp:bbb\"\nkp_schema: kp-note/v1\ntitle: Async rust\n---\n\
                 rust database embedded sqlite storage engine queries indexes async\n",
            ),
            (
                "cooking/bread.md",
                "---\nkp_id: \"kp:ccc\"\nkp_schema: kp-note/v1\ntitle: Bread\n---\n\
                 sourdough flour hydration crumb oven steam levain proofing\n",
            ),
        ];
        for (path, content) in notes {
            let note = Note::parse(path, content).expect("parses");
            idx.upsert_note(&note, &e, p).expect("upsert");
        }
        (idx, e)
    }

    #[test]
    fn get_note_round_trips_columns() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (idx, _) = seeded(dir.path());
        let reader = idx.reader().expect("reader");

        let note = reader.get_note("kp:aaa").expect("query").expect("present");
        assert_eq!(note.kp_id, "kp:aaa");
        assert_eq!(note.path, "rust/db.md");
        assert_eq!(note.title, "Rust databases");
        assert_eq!(note.tags, vec!["rust", "databases"]);
        assert_eq!(note.source.as_deref(), Some("https://example.com/rust-db"));
        assert_eq!(note.updated.as_deref(), Some("2026-07-01T00:00:00Z"));
        assert!(note.body.contains("storage engine"));
        assert!(note.ingested_at.ends_with('Z'));

        assert_eq!(reader.get_note("kp:zzz").expect("query"), None);
    }

    #[test]
    fn related_finds_topical_neighbors_and_excludes_self() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (idx, _) = seeded(dir.path());
        let reader = idx.reader().expect("reader");

        let hits = reader.related("kp:aaa", 2).expect("query");
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|h| h.kp_id != "kp:aaa"), "self excluded");
        // The other rust note beats the bread note.
        assert_eq!(hits[0].kp_id, "kp:bbb", "hits: {hits:?}");
        for pair in hits.windows(2) {
            assert!(pair[0].score >= pair[1].score, "sorted by score desc");
        }

        // Unknown id → empty, not an error.
        assert_eq!(reader.related("kp:zzz", 5).expect("query"), Vec::new());
    }

    #[test]
    fn recent_since_filters_by_cutoff_and_namespace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut idx, e) = seeded(dir.path());
        // Backdate one note past any cutoff we'll use.
        idx.conn
            .execute(
                "UPDATE notes SET ingested_at = '2020-01-01T00:00:00Z' WHERE kp_id = 'kp:ccc'",
                [],
            )
            .expect("backdate");
        let plain = Note::parse("plain.md", "no frontmatter body\n").expect("parses");
        idx.upsert_note(&plain, &e, ChunkParams::default())
            .expect("upsert");
        let reader = idx.reader().expect("reader");

        let all = reader
            .recent_since("2026-01-01T00:00:00Z", None, 10)
            .expect("query");
        let ids: Vec<&str> = all.iter().map(|n| n.kp_id.as_str()).collect();
        assert!(ids.contains(&"kp:aaa"));
        assert!(ids.contains(&"path:plain.md"));
        assert!(!ids.contains(&"kp:ccc"), "backdated note filtered out");

        let kp_only = reader
            .recent_since("2026-01-01T00:00:00Z", Some("kp"), 10)
            .expect("query");
        assert!(kp_only.iter().all(|n| n.kp_id.starts_with("kp:")));
        assert!(!kp_only.is_empty());

        let limited = reader
            .recent_since("2020-01-01T00:00:00Z", None, 2)
            .expect("query");
        assert_eq!(limited.len(), 2);
    }

    #[test]
    fn latest_digest_prefers_the_log_then_falls_back_to_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut idx, e) = seeded(dir.path());
        let reader = idx.reader().expect("reader");
        assert_eq!(reader.latest_digest("digests").expect("query"), None);

        // Two date-named digests under the digest dir.
        for (path, id) in [
            ("digests/2026-07-01.md", "kp:d1"),
            ("digests/2026-07-02.md", "kp:d2"),
        ] {
            let content = format!(
                "---\nkp_id: \"{id}\"\nkp_schema: kp-note/v1\ntitle: Digest\n---\ndigest body\n"
            );
            let note = Note::parse(path, content.as_str()).expect("parses");
            idx.upsert_note(&note, &e, ChunkParams::default())
                .expect("upsert");
        }
        let reader = idx.reader().expect("reader");
        let latest = reader
            .latest_digest("digests")
            .expect("query")
            .expect("fallback finds the newest path");
        assert_eq!(latest.kp_id, "kp:d2");

        // Once the librarian logs a digest, the log wins.
        idx.conn
            .execute(
                "INSERT INTO digest_log (digest_date, kp_id, created)
                 VALUES ('2026-07-01', 'kp:d1', '2026-07-01T06:00:00Z')",
                [],
            )
            .expect("log row");
        let latest = idx
            .reader()
            .expect("reader")
            .latest_digest("digests")
            .expect("query")
            .expect("present");
        assert_eq!(latest.kp_id, "kp:d1");
    }

    #[test]
    fn reader_links_from_matches_writer_view() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut idx, _) = seeded(dir.path());
        idx.add_link("kp:aaa", "kp:bbb", "wikilink").expect("link");
        idx.add_link("kp:aaa", "kp:ccc", "markdown").expect("link");
        let reader = idx.reader().expect("reader");
        assert_eq!(
            reader.links_from("kp:aaa").expect("query"),
            vec![
                ("kp:bbb".to_owned(), "wikilink".to_owned()),
                ("kp:ccc".to_owned(), "markdown".to_owned()),
            ]
        );
        assert_eq!(reader.links_from("kp:bbb").expect("query"), Vec::new());
    }
}
