//! Hybrid retrieval: BM25 (FTS5) + cosine (sqlite-vec), fused with
//! reciprocal rank fusion.
//!
//! Three entry points on [`IndexReader`]: [`IndexReader::fts_search`],
//! [`IndexReader::vec_search`], and [`IndexReader::hybrid_search`]. All
//! return note-level hits — chunk matches are collapsed to their best
//! chunk — as `(kp_id, title, path, snippet, score)`, higher score better.

use std::collections::HashMap;

use rusqlite::params;

use crate::db::IndexReader;
use crate::embed::Embedder;
use crate::error::IndexError;

/// One search result.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub kp_id: String,
    pub title: String,
    pub path: String,
    /// Match context: FTS `snippet()` (with `[`…`]` highlights) when the
    /// note matched full-text, otherwise the best-matching chunk's text.
    pub snippet: String,
    /// Leg-specific score, higher is better: `-bm25` for FTS, cosine
    /// similarity for vectors, RRF mass for hybrid.
    pub score: f64,
}

/// RRF constant — the standard 60 from the original Cormack et al. recipe;
/// large enough that a few rank-1 appearances beat one lucky leg.
const RRF_K: f64 = 60.0;

/// How much deeper than `k` each leg reaches before fusion. A note buried
/// at rank k+1 in both legs can still out-fuse a note found by only one.
const POOL_FACTOR: usize = 3;

/// Escape user text into an FTS5 MATCH expression: alphanumeric runs
/// become quoted terms, OR-joined (recall-friendly — ranking, not boolean
/// logic, does the precision work). FTS5 operators, parens, and quotes in
/// user input never reach the parser.
fn fts_query(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

impl IndexReader {
    /// Full-text leg: BM25 over title+body, snippets from FTS highlight.
    pub fn fts_search(&self, query: &str, k: usize) -> Result<Vec<SearchHit>, IndexError> {
        let Some(match_expr) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare(
            "SELECT n.kp_id, n.title, n.path,
                    snippet(notes_fts, -1, '[', ']', '…', 12),
                    bm25(notes_fts)
             FROM notes_fts
             JOIN notes n ON n.rowid = notes_fts.rowid
             WHERE notes_fts MATCH ?1
             ORDER BY bm25(notes_fts)
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![match_expr, k as i64], |r| {
            Ok(SearchHit {
                kp_id: r.get(0)?,
                title: r.get(1)?,
                path: r.get(2)?,
                snippet: r.get(3)?,
                // bm25() is lower-is-better (negative); flip so higher wins.
                score: -r.get::<_, f64>(4)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Vector leg: embed the query, KNN over chunk vectors (cosine, via
    /// sqlite-vec), collapse to best chunk per note. Score = cosine
    /// similarity (1 − cosine distance).
    pub fn vec_search(
        &self,
        embedder: &dyn Embedder,
        query: &str,
        k: usize,
    ) -> Result<Vec<SearchHit>, IndexError> {
        self.check_embedder(embedder)?;
        let qvec = embedder.embed_one(query)?;
        if qvec.iter().all(|x| *x == 0.0) {
            return Ok(Vec::new()); // empty/stopword-only query
        }
        let mut blob = Vec::with_capacity(qvec.len() * 4);
        for x in &qvec {
            blob.extend_from_slice(&x.to_le_bytes());
        }
        // Over-fetch chunk neighbors: several top chunks may belong to the
        // same note, and we want k distinct NOTES.
        let k_chunks = k.saturating_mul(POOL_FACTOR).max(k) as i64;
        let mut stmt = self.conn.prepare(
            "SELECT n.kp_id, n.title, n.path, c.text, v.distance
             FROM (SELECT rowid, distance FROM vec_chunks
                   WHERE embedding MATCH ?1 AND k = ?2) v
             JOIN chunks c ON c.id = v.rowid
             JOIN notes n ON n.kp_id = c.note
             ORDER BY v.distance",
        )?;
        let rows = stmt.query_map(params![blob, k_chunks], |r| {
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
                snippet: clip(&chunk_text, 160),
                score: 1.0 - distance,
            });
        }
        Ok(hits)
    }

    /// Hybrid: reciprocal rank fusion over the FTS and vector legs.
    /// `score(note) = Σ_legs 1 / (60 + rank)`; FTS snippets win over chunk
    /// text when a note appears in both legs.
    pub fn hybrid_search(
        &self,
        embedder: &dyn Embedder,
        query: &str,
        k: usize,
    ) -> Result<Vec<SearchHit>, IndexError> {
        let pool = k.saturating_mul(POOL_FACTOR).max(k);
        let fts = self.fts_search(query, pool)?;
        let vec = self.vec_search(embedder, query, pool)?;

        let mut fused: HashMap<String, SearchHit> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for (leg_priority, leg) in [fts, vec].into_iter().enumerate() {
            for (rank, hit) in leg.into_iter().enumerate() {
                let rrf = 1.0 / (RRF_K + rank as f64 + 1.0);
                match fused.get_mut(&hit.kp_id) {
                    Some(existing) => {
                        existing.score += rrf;
                        // leg_priority 0 = FTS: its highlighted snippet is
                        // already in place; never downgrade to chunk text.
                        debug_assert!(leg_priority <= 1);
                    }
                    None => {
                        order.push(hit.kp_id.clone());
                        fused.insert(hit.kp_id.clone(), SearchHit { score: rrf, ..hit });
                    }
                }
            }
        }
        let mut hits: Vec<SearchHit> = order
            .into_iter()
            .map(|id| fused.remove(&id).expect("id recorded exactly once"))
            .collect();
        // Stable, deterministic ordering: fused score desc, kp_id as the
        // total tie-break.
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .expect("RRF scores are finite")
                .then_with(|| a.kp_id.cmp(&b.kp_id))
        });
        hits.truncate(k);
        Ok(hits)
    }
}

/// Clip to at most `max` chars on a char boundary, with ellipsis.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let clipped: String = s.chars().take(max).collect();
    format!("{clipped}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts_query_escapes_operators_and_quotes() {
        assert_eq!(
            fts_query("hello world"),
            Some("\"hello\" OR \"world\"".to_owned())
        );
        // FTS5 syntax in user input is neutralized, not executed.
        assert_eq!(
            fts_query("NEAR(a b)"),
            Some("\"NEAR\" OR \"a\" OR \"b\"".to_owned())
        );
        assert_eq!(fts_query("\"quoted\""), Some("\"quoted\"".to_owned()));
        assert_eq!(fts_query("  \t "), None);
        assert_eq!(fts_query("?!"), None);
    }

    #[test]
    fn clip_respects_char_boundaries() {
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("ééééé", 3), "ééé…");
    }
}
