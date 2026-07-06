//! End-to-end digest runs against a real vault + index: ingest → digest
//! run (auto) → the digest note exists in the vault via a proposals/v1
//! proposal; a re-run with the same clock is a no-op; identical
//! environments produce byte-identical digests.
//!
//! Hermetic: hash embedder, temp dirs, injected clock.

use std::path::Path;

use curator_core::note::{Frontmatter, Note};
use curator_core::{KpConfig, KpId, ProposalStatus, Vault, list_proposals};
use curator_index::{HashEmbedder, IndexReader};
use curator_librarian::{is_uuid7, run_digest};

/// 2026-07-03T09:15:00Z — the injected clock.
const NOW: u64 = 1_783_070_100;

fn build_env(dir: &Path) -> KpConfig {
    let vault = dir.join("vault");
    std::fs::create_dir_all(&vault).expect("mkdir vault");
    let notes: &[(&str, &str)] = &[
        (
            "now.md",
            "# Now\n\nrust database sqlite embedded storage engine work\n",
        ),
        (
            "rust/db.md",
            "---\nkp_id: \"kp:0197aaaa-0000-7000-8000-000000000001\"\nkp_schema: kp-note/v1\n\
             title: Rust databases\ntags: [rust, databases]\ncreated: 2026-07-01T00:00:00Z\n---\n\
             Embedded sqlite storage engine notes for rust database work.\n",
        ),
        (
            "rust/async.md",
            "---\nkp_id: \"kp:0197aaaa-0000-7000-8000-000000000002\"\nkp_schema: kp-note/v1\n\
             title: Async rust\ntags: [rust]\ncreated: 2026-06-05T00:00:00Z\n---\n\
             Async rust database drivers and sqlite storage.\n",
        ),
        (
            "cooking/bread.md",
            "---\nkp_id: \"kp:0197aaaa-0000-7000-8000-000000000003\"\nkp_schema: kp-note/v1\n\
             title: Bread\ntags: [cooking]\ncreated: 2026-07-02T00:00:00Z\n---\n\
             Sourdough hydration and proofing schedule.\n",
        ),
    ];
    for (path, content) in notes {
        let abs = vault.join(path);
        std::fs::create_dir_all(abs.parent().expect("parent")).expect("mkdir");
        std::fs::write(abs, content).expect("write note");
    }
    let toml = format!(
        "schema = \"kp-config/v1\"\n\
         [vault]\npath = \"{}\"\n\
         [index]\npath = \"{}\"\nembedder = \"hash\"\n",
        vault.display(),
        dir.join("index.db").display(),
    );
    KpConfig::from_toml_str(&toml).expect("config parses")
}

#[test]
fn ingest_then_auto_digest_lands_the_note_and_reruns_are_noops() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = build_env(dir.path());
    let e = HashEmbedder::default();
    let ingested = curator_ingest::ingest(&config, &e).expect("ingest");
    assert_eq!(ingested.ingested, 4);

    let report = run_digest(&config, &e, NOW, true).expect("digest runs");
    assert_eq!(report.date, "2026-07-03");
    assert_eq!(report.note_path, "digests/2026-07-03.md");
    assert!(
        report.applied,
        "auto gate must admit the digest: {report:?}"
    );
    assert_eq!(report.skipped, None);
    assert_eq!(report.candidates, 3, "now.md (the anchor) is excluded");
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);

    // The note landed in the vault, shaped as a kp-note digest.
    let vault = Vault::open(config.vault_path()).expect("vault");
    let content = vault.read("digests/2026-07-03.md").expect("digest exists");
    let note = Note::parse("digests/2026-07-03.md", &content).expect("parses");
    let Frontmatter::Kp(fm) = &note.frontmatter else {
        panic!("digest must carry kp frontmatter");
    };
    let KpId::Kp(uuid) = &fm.kp_id else {
        panic!("digest identity must be kp:<uuidv7>, got {}", fm.kp_id);
    };
    assert!(is_uuid7(uuid), "{uuid}");
    assert_eq!(fm.title, "Daily digest 2026-07-03");
    assert_eq!(fm.tags, vec!["digest"]);
    assert_eq!(fm.created.as_deref(), Some("2026-07-03T09:15:00Z"));
    assert!(
        note.body.contains("[[rust/db|Rust databases]]"),
        "{}",
        note.body
    );

    // It rode a proposal, now stamped applied…
    let proposals = list_proposals(&vault, ".kp/proposals").expect("lists");
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].status, ProposalStatus::Applied);
    assert_eq!(proposals[0].author, "kp-librarian");

    // …and the index serves it as the latest digest without a re-ingest.
    let reader = IndexReader::open(config.index_path()).expect("reader");
    let entry = reader
        .last_digest_entry()
        .expect("query")
        .expect("digest logged");
    assert_eq!(entry.digest_date, "2026-07-03");
    let latest = reader
        .latest_digest("digests")
        .expect("query")
        .expect("served");
    assert_eq!(latest.kp_id, format!("kp:{uuid}"));

    // Re-run with the same clock: a no-op, no duplicate anything.
    let second = run_digest(&config, &e, NOW, true).expect("second run");
    assert!(second.skipped.is_some(), "{second:?}");
    assert_eq!(second.proposal_id, None);
    assert_eq!(
        list_proposals(&vault, ".kp/proposals")
            .expect("lists")
            .len(),
        1,
        "no duplicate proposal"
    );
    assert_eq!(
        vault.read("digests/2026-07-03.md").expect("read"),
        content,
        "the digest note is untouched"
    );
}

#[test]
fn identical_environments_produce_byte_identical_digests() {
    let e = HashEmbedder::default();
    let render = || {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = build_env(dir.path());
        curator_ingest::ingest(&config, &e).expect("ingest");
        let report = run_digest(&config, &e, NOW, true).expect("digest");
        assert!(report.applied);
        Vault::open(config.vault_path())
            .expect("vault")
            .read(&report.note_path)
            .expect("digest exists")
    };
    assert_eq!(render(), render(), "clock injected → bytes reproducible");
}

#[test]
fn missing_now_md_warns_and_falls_back_to_recency() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = build_env(dir.path());
    std::fs::remove_file(config.vault_path().join("now.md")).expect("drop anchor");
    let e = HashEmbedder::default();
    curator_ingest::ingest(&config, &e).expect("ingest");

    let report = run_digest(&config, &e, NOW, true).expect("digest runs");
    assert!(report.applied);
    assert!(
        report.warnings.iter().any(|w| w.contains("now.md")),
        "{:?}",
        report.warnings
    );
    let body = Vault::open(config.vault_path())
        .expect("vault")
        .read(&report.note_path)
        .expect("digest");
    assert!(body.contains("recency-only ranking"), "{body}");
}
