//! Blue/green epoch machinery, end to end: full rebuilds swap atomically,
//! incremental ingest updates in place, and verification failures never
//! touch the serving file. Hermetic: HashEmbedder only, temp dirs only.

use std::fs;
use std::path::Path;

use kp_core::KpConfig;
use kp_index::{ChunkParams, HashEmbedder, Index, IndexError, IndexReader, build_epoch};

fn config_for(root: &Path) -> KpConfig {
    let vault = root.join("vault");
    fs::create_dir_all(&vault).expect("mkdir vault");
    let raw = format!(
        "schema = \"kp-config/v1\"\n\
         [vault]\npath = \"{}\"\n\
         [index]\npath = \"{}\"\nembedder = \"hash\"\nchunk_tokens = 16\nchunk_overlap = 4\n",
        vault.display(),
        root.join("state/index.db").display(),
    );
    KpConfig::from_toml_str(&raw).expect("test config parses")
}

fn write_note(cfg: &KpConfig, rel: &str, body: &str) {
    let path = cfg.vault_path().join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, body).expect("write note");
}

#[test]
fn first_epoch_builds_from_scratch_and_serves() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = config_for(dir.path());
    let e = HashEmbedder::new(64);
    write_note(&cfg, "alpha.md", "# Alpha\nrust sqlite embedded database\n");
    write_note(&cfg, "sub/beta.md", "# Beta\ncooking pasta tomato basil\n");

    let report = build_epoch(&cfg, &e).expect("first build");
    assert_eq!(report.epoch, 1);
    assert_eq!(report.notes_indexed, 2);
    assert_eq!(report.notes_skipped, 0);
    assert!(cfg.index_path().exists());
    assert!(
        !index_next_path(&cfg).exists(),
        ".next must be gone after the swap"
    );

    let reader = IndexReader::open(cfg.index_path()).expect("open");
    assert_eq!(reader.meta().epoch, 1);
    assert_eq!(reader.meta().embedder_id, "hash");
    assert_eq!(reader.meta().dims, 64);
    let hits = reader.fts_search("sqlite", 5).expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].kp_id, "path:alpha.md");
    assert_eq!(hits[0].title, "alpha");
    assert_eq!(hits[0].path, "alpha.md");
}

#[test]
fn rebuild_bumps_the_epoch_and_reflects_vault_changes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = config_for(dir.path());
    let e = HashEmbedder::new(64);
    write_note(&cfg, "keep.md", "stays the same forever\n");
    write_note(&cfg, "gone.md", "will be deleted xylophone\n");
    build_epoch(&cfg, &e).expect("first build");

    fs::remove_file(cfg.vault_path().join("gone.md")).expect("rm");
    write_note(&cfg, "new.md", "freshly written quartz note\n");
    let report = build_epoch(&cfg, &e).expect("rebuild");
    assert_eq!(report.epoch, 2);
    assert_eq!(report.notes_indexed, 2);

    let reader = IndexReader::open(cfg.index_path()).expect("open");
    assert_eq!(reader.meta().epoch, 2);
    assert!(
        reader
            .fts_search("xylophone", 5)
            .expect("search")
            .is_empty()
    );
    let hits = reader.fts_search("quartz", 5).expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].kp_id, "path:new.md");
}

#[test]
fn stale_next_file_from_a_crashed_build_is_discarded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = config_for(dir.path());
    let e = HashEmbedder::new(64);
    write_note(&cfg, "a.md", "real content\n");
    fs::create_dir_all(cfg.index_path().parent().expect("parent")).expect("mkdir");
    fs::write(index_next_path(&cfg), b"garbage from a crashed build").expect("write");

    let report = build_epoch(&cfg, &e).expect("build survives stale .next");
    assert_eq!(report.epoch, 1);
    assert!(!index_next_path(&cfg).exists());
    IndexReader::open(cfg.index_path()).expect("the swapped-in file is a real index");
}

#[test]
fn unparseable_and_duplicate_notes_are_skipped_not_fatal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = config_for(dir.path());
    let e = HashEmbedder::new(64);
    write_note(&cfg, "good.md", "fine note\n");
    // Unterminated frontmatter: parse error.
    write_note(&cfg, "broken.md", "---\nkp_id: \"kp:x\"\nnever closed\n");
    // Two files claiming one identity: second (path-sorted) is skipped.
    let dup = "---\nkp_id: \"kp:dup\"\nkp_schema: kp-note/v1\ntitle: D\n---\nbody\n";
    write_note(&cfg, "dup-a.md", dup);
    write_note(&cfg, "dup-b.md", dup);

    let report = build_epoch(&cfg, &e).expect("build");
    assert_eq!(report.notes_indexed, 2, "good.md + dup-a.md");
    assert_eq!(report.notes_skipped, 2, "broken.md + dup-b.md");
    let reader = IndexReader::open(cfg.index_path()).expect("open");
    let hits = reader.fts_search("body", 5).expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "dup-a.md", "first path-sorted claimant wins");
}

#[test]
fn incremental_ingest_updates_in_place_same_epoch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = config_for(dir.path());
    let e = HashEmbedder::new(64);
    write_note(&cfg, "a.md", "original wording here\n");
    build_epoch(&cfg, &e).expect("build");

    // The incremental path: open the SERVING file and upsert in place.
    let mut idx = Index::open(cfg.index_path(), &e).expect("open");
    let note = kp_core::Note::parse("a.md", "rewritten amended wording\n").expect("parses");
    idx.upsert_note(&note, &e, ChunkParams::from_config(&cfg.index))
        .expect("upsert");
    let epoch_after = idx.meta().epoch;
    idx.close().expect("close");

    let reader = IndexReader::open(cfg.index_path()).expect("reopen");
    assert_eq!(
        reader.meta().epoch,
        epoch_after,
        "incremental ingest never bumps the epoch"
    );
    assert_eq!(reader.meta().epoch, 1);
    assert!(reader.fts_search("original", 5).expect("search").is_empty());
    assert_eq!(reader.fts_search("rewritten", 5).expect("search").len(), 1);
}

#[test]
fn swapping_embedders_requires_a_rebuild_then_works() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = config_for(dir.path());
    write_note(&cfg, "a.md", "some note\n");
    build_epoch(&cfg, &HashEmbedder::new(64)).expect("build @64");

    // Opening with a different-dims embedder: refused, demands rebuild.
    let err = Index::open(cfg.index_path(), &HashEmbedder::new(128)).unwrap_err();
    assert!(matches!(err, IndexError::EmbedderMismatch { .. }));

    // The demanded rebuild IS the fix: blue/green swap re-embeds everything.
    let report = build_epoch(&cfg, &HashEmbedder::new(128)).expect("rebuild @128");
    assert_eq!(report.epoch, 2);
    let reader = IndexReader::open(cfg.index_path()).expect("open");
    assert_eq!(reader.meta().dims, 128);
    Index::open(cfg.index_path(), &HashEmbedder::new(128)).expect("now opens");
}

#[test]
fn a_failed_build_leaves_the_serving_epoch_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = config_for(dir.path());
    let e = HashEmbedder::new(64);
    write_note(&cfg, "a.md", "serving content emerald\n");
    build_epoch(&cfg, &e).expect("first build");

    // Nuke the vault dir: the next build must fail before any swap...
    fs::remove_dir_all(cfg.vault_path()).expect("rm vault");
    build_epoch(&cfg, &e).expect_err("no vault, no build");

    // ...and the serving index still answers.
    let reader = IndexReader::open(cfg.index_path()).expect("still serving");
    assert_eq!(reader.meta().epoch, 1);
    assert_eq!(reader.fts_search("emerald", 5).expect("search").len(), 1);
}

fn index_next_path(cfg: &KpConfig) -> std::path::PathBuf {
    let mut p = cfg.index_path().into_os_string();
    p.push(".next");
    p.into()
}
