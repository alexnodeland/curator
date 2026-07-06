//! Retrieval behavior over a seeded corpus, with the deterministic
//! HashEmbedder: known-similar notes must rank first on every leg, and
//! hybrid fusion must reward notes both legs agree on. Hermetic.

use curator_core::Note;
use curator_index::{ChunkParams, HashEmbedder, Index, IndexReader};

/// Seeded corpus: three database notes, two unrelated, one bridging.
const CORPUS: &[(&str, &str, &str)] = &[
    (
        "notes/sqlite-internals.md",
        "SQLite Internals",
        "sqlite embedded database storage engine btree pages write ahead log \
         vacuum checkpoint pragma journal wal reader writer",
    ),
    (
        "notes/vector-indexes.md",
        "Vector Indexes",
        "vector embedding index cosine similarity nearest neighbor search \
         quantization recall latency tradeoff",
    ),
    (
        "notes/duckdb-notes.md",
        "DuckDB Notes",
        "embedded analytical database columnar storage engine olap query \
         vectorized execution parquet",
    ),
    (
        "notes/sourdough.md",
        "Sourdough Log",
        "sourdough starter flour hydration levain bulk ferment oven spring \
         crumb scoring banneton",
    ),
    (
        "notes/marathon.md",
        "Marathon Plan",
        "marathon training long run tempo intervals taper carbohydrate \
         pacing negative split",
    ),
    (
        "notes/sqlite-vec-eval.md",
        "Evaluating sqlite for vectors",
        "evaluating sqlite as an embedded vector database cosine search \
         inside one storage file",
    ),
];

fn seeded_index(dir: &std::path::Path) -> (Index, HashEmbedder) {
    let e = HashEmbedder::new(128);
    let mut idx = Index::create(dir.join("index.db"), &e, 1).expect("create");
    let params = ChunkParams {
        tokens: 12,
        overlap: 3,
    };
    for (path, title, body) in CORPUS {
        let raw = format!(
            "---\nkp_id: \"kp:{path}\"\nkp_schema: kp-note/v1\ntitle: \"{title}\"\n---\n{body}\n"
        );
        let note = Note::parse(*path, &raw).expect("parses");
        idx.upsert_note(&note, &e, params).expect("upsert");
    }
    (idx, e)
}

fn reader(idx: &Index) -> IndexReader {
    idx.reader().expect("reader")
}

#[test]
fn fts_ranks_exact_term_matches_first_with_highlighted_snippets() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (idx, _e) = seeded_index(dir.path());
    let r = reader(&idx);

    let hits = r.fts_search("sourdough hydration", 10).expect("search");
    assert!(!hits.is_empty());
    assert_eq!(hits[0].kp_id, "kp:notes/sourdough.md");
    assert_eq!(hits[0].title, "Sourdough Log");
    assert_eq!(hits[0].path, "notes/sourdough.md");
    assert!(
        hits[0].snippet.contains("[sourdough]") || hits[0].snippet.contains("[hydration]"),
        "FTS snippet must highlight the match: {:?}",
        hits[0].snippet
    );
    assert!(hits[0].score > 0.0, "flipped bm25 is positive for matches");

    // Multi-match ranking: both sqlite notes beat the rest for "sqlite".
    let hits = r.fts_search("sqlite", 10).expect("search");
    let ids: Vec<&str> = hits.iter().map(|h| h.kp_id.as_str()).collect();
    assert_eq!(hits.len(), 2);
    assert!(ids.contains(&"kp:notes/sqlite-internals.md"));
    assert!(ids.contains(&"kp:notes/sqlite-vec-eval.md"));

    // No match, no rows; k caps output.
    assert!(
        r.fts_search("zzzunknownzzz", 10)
            .expect("search")
            .is_empty()
    );
    assert_eq!(
        r.fts_search("database OR storage OR embedded", 2)
            .expect("search")
            .len(),
        2
    );
}

#[test]
fn vec_search_ranks_token_overlapping_notes_first() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (idx, e) = seeded_index(dir.path());
    let r = reader(&idx);

    let hits = r
        .vec_search(&e, "embedded database storage engine", 6)
        .expect("search");
    assert!(hits.len() >= 3);
    // The two storage-engine notes must occupy the top ranks; the hobby
    // notes must rank strictly below them.
    let top2: Vec<&str> = hits[..2].iter().map(|h| h.kp_id.as_str()).collect();
    assert!(
        top2.contains(&"kp:notes/sqlite-internals.md")
            && top2.contains(&"kp:notes/duckdb-notes.md"),
        "expected the storage notes on top, got {top2:?}"
    );
    let pos = |id: &str| hits.iter().position(|h| h.kp_id == id);
    for hobby in ["kp:notes/sourdough.md", "kp:notes/marathon.md"] {
        if let Some(p) = pos(hobby) {
            assert!(p > 1, "{hobby} outranked a storage note: {hits:?}");
            assert!(
                hits[p].score < hits[0].score - 0.15,
                "cosine separation too small: {hits:?}"
            );
        }
    }
    // Scores are cosine similarities: within [-1, 1], best first.
    assert!(hits.windows(2).all(|w| w[0].score >= w[1].score));
    assert!(hits[0].score <= 1.0 + 1e-6 && hits[0].score > 0.3);
    // Snippet falls back to chunk text on the vector leg.
    assert!(!hits[0].snippet.is_empty());

    // One note per result even though notes have several chunks.
    let mut ids: Vec<&str> = hits.iter().map(|h| h.kp_id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), hits.len(), "chunk hits must collapse to notes");
}

#[test]
fn vec_search_rejects_a_mismatched_query_embedder() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (idx, _e) = seeded_index(dir.path());
    let r = reader(&idx);
    // Same family, wrong dims: querying with it would compare apples to
    // oranges — refused, demanding the epoch rebuild.
    let err = r
        .vec_search(&HashEmbedder::new(64), "anything", 5)
        .unwrap_err();
    assert!(matches!(
        err,
        curator_index::IndexError::EmbedderMismatch { .. }
    ));
}

#[test]
fn hybrid_fuses_both_legs_and_rewards_agreement() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (idx, e) = seeded_index(dir.path());
    let r = reader(&idx);

    // "sqlite vector cosine database": sqlite-vec-eval.md is the one note
    // strong on BOTH legs (exact terms + token overlap) — RRF must put it
    // first, above notes that win only one leg.
    let hits = r
        .hybrid_search(&e, "sqlite vector cosine database", 5)
        .expect("search");
    assert!(hits.len() >= 3);
    assert_eq!(hits[0].kp_id, "kp:notes/sqlite-vec-eval.md", "got {hits:?}");

    // Fusion means single-leg winners still show up (recall from both legs).
    let ids: Vec<&str> = hits.iter().map(|h| h.kp_id.as_str()).collect();
    assert!(
        ids.contains(&"kp:notes/vector-indexes.md"),
        "vector-leg note missing: {ids:?}"
    );
    assert!(
        ids.contains(&"kp:notes/sqlite-internals.md"),
        "fts-leg note missing: {ids:?}"
    );

    // Off-topic notes stay out of the top ranks.
    assert!(!hits[..2].iter().any(|h| h.kp_id.contains("sourdough")));

    // k is respected, scores are descending RRF mass.
    assert!(hits.len() <= 5);
    assert!(hits.windows(2).all(|w| w[0].score >= w[1].score));
    assert!(
        hits[0].score <= 2.0 / 61.0 + 1e-9,
        "RRF mass is bounded by both legs at rank 1"
    );

    // Determinism: same query, same ranking, every time.
    let again = r
        .hybrid_search(&e, "sqlite vector cosine database", 5)
        .expect("search");
    assert_eq!(hits, again);
}

#[test]
fn hybrid_prefers_fts_snippets_when_a_note_matches_both_legs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (idx, e) = seeded_index(dir.path());
    let r = reader(&idx);
    let hits = r
        .hybrid_search(&e, "sqlite cosine vector", 5)
        .expect("search");
    let eval = hits
        .iter()
        .find(|h| h.kp_id == "kp:notes/sqlite-vec-eval.md")
        .expect("bridging note present");
    assert!(
        eval.snippet.contains('[') && eval.snippet.contains(']'),
        "both-legs note must carry the FTS-highlighted snippet: {:?}",
        eval.snippet
    );
}

#[test]
fn empty_and_unmatchable_queries_return_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (idx, e) = seeded_index(dir.path());
    let r = reader(&idx);
    assert!(r.fts_search("", 5).expect("search").is_empty());
    assert!(r.vec_search(&e, "", 5).expect("search").is_empty());
    assert!(r.hybrid_search(&e, "", 5).expect("search").is_empty());
    assert!(r.hybrid_search(&e, "?!\"(", 5).expect("search").is_empty());
}
