//! End-to-end ingest over the checked-in fixture vault: a small realistic
//! corpus (plain notes, a kp-frontmatter note, a Curio export with managed
//! region + manifest + events, wikilinks, `.kpignore`). The fixture tree
//! is READ-ONLY — ingest writes only to the index in a tempdir.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use kp_core::KpConfig;
use kp_index::embed::EmbedError;
use kp_index::{Embedder, HashEmbedder, Index};
use kp_ingest::ingest;

const CURIO_KP_ID: &str = "curio:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d";
const RUST_NOTES_KP_ID: &str = "kp:0197c001-2222-7bbb-8ccc-000000000001";

/// Wraps the hash embedder and counts the expensive path.
struct CountingEmbedder {
    inner: HashEmbedder,
    calls: AtomicUsize,
    texts: AtomicUsize,
}

impl CountingEmbedder {
    fn new() -> Self {
        Self {
            inner: HashEmbedder::new(64),
            calls: AtomicUsize::new(0),
            texts: AtomicUsize::new(0),
        }
    }
}

impl Embedder for CountingEmbedder {
    fn id(&self) -> &str {
        self.inner.id()
    }
    fn dims(&self) -> usize {
        self.inner.dims()
    }
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.texts.fetch_add(texts.len(), Ordering::SeqCst);
        self.inner.embed(texts)
    }
}

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn config(index_path: &std::path::Path) -> KpConfig {
    let toml = format!(
        "schema = \"kp-config/v1\"\n\
         [vault]\npath = \"{vault}\"\n\
         [index]\npath = \"{index}\"\nembedder = \"hash\"\nchunk_tokens = 64\nchunk_overlap = 8\n\
         [curio]\nenabled = true\nevents_dir = \"{events}\"\n",
        vault = fixture("fixtures/vault").display(),
        index = index_path.display(),
        events = fixture("fixtures/events").display(),
    );
    KpConfig::from_toml_str(&toml).expect("fixture config parses")
}

#[test]
fn full_pipeline_over_the_fixture_vault() {
    let dir = tempfile::tempdir().expect("tempdir");
    let index_path = dir.path().join("kp").join("index.db");
    let cfg = config(&index_path);

    // ---- First run: everything is new. ----
    let embedder = CountingEmbedder::new();
    let report = ingest(&cfg, &embedder).expect("first ingest");

    assert_eq!(report.scanned, 6, "6 eligible notes walked");
    assert_eq!(report.ignored, 2, ".kpignore drops drafts/ and *.tmp.md");
    assert_eq!(report.curio_notes, 1);
    assert_eq!(report.skipped, 1, "the schema-violating Curio note");
    assert_eq!(report.ingested, 5);
    assert_eq!(report.unchanged, 0);
    assert_eq!(report.removed, 0);
    assert_eq!(report.links, 6);
    // Batch-first: ONE embed call for the whole run.
    assert_eq!(embedder.calls.load(Ordering::SeqCst), 1);
    assert!(embedder.texts.load(Ordering::SeqCst) >= 5);

    let events = report.events.as_ref().expect("curio enabled");
    assert_eq!(events.files, 3);
    assert_eq!(events.folded, 6);
    assert_eq!(events.duplicates, 0);
    assert_eq!(events.malformed, 0);

    // ---- The indexed state. ----
    let probe = HashEmbedder::new(64);
    let index = Index::open(&index_path, &probe).expect("open");

    // Identity derivation: explicit kp_id > curio adapter > path fallback.
    let curio_state = index
        .note_state(CURIO_KP_ID)
        .expect("query")
        .expect("curio note indexed under its adapted identity");
    assert_eq!(curio_state.path, "curio/rust-async-patterns.md");
    let token = curio_state.checksum.as_deref().expect("token recorded");
    assert!(token.starts_with("sha256:"), "{token}");
    assert_ne!(
        token, "sha256:4a44dc15364204a80fe80e9039455cc1608281820fe2b24f1e5233ade6af1dd5",
        "change detection keys on the FULL note token, never the \
         producer-declared managed-region checksum"
    );
    assert!(
        index.note_state(RUST_NOTES_KP_ID).expect("query").is_some(),
        "explicit kp_id wins"
    );
    assert!(
        index
            .note_state("path:notes/databases.md")
            .expect("query")
            .is_some(),
        "plain notes fall back to path identity"
    );
    // Ignored + invalid notes never land.
    for absent in [
        "path:drafts/wip.md",
        "path:scratch.tmp.md",
        "path:curio/broken-import.md",
    ] {
        assert!(
            index.note_state(absent).expect("query").is_none(),
            "{absent} must not be indexed"
        );
    }

    // Links: wikilinks + md links, resolved across identity namespaces.
    assert_eq!(
        index.links_from("path:now.md").expect("query"),
        vec![
            (RUST_NOTES_KP_ID.to_owned(), "wikilink".to_owned()),
            ("path:notes/databases.md".to_owned(), "wikilink".to_owned()),
        ]
    );
    assert_eq!(
        index.links_from(RUST_NOTES_KP_ID).expect("query"),
        vec![
            ("path:guides/chunking.md".to_owned(), "markdown".to_owned()),
            ("path:notes/databases.md".to_owned(), "wikilink".to_owned()),
        ]
    );
    assert_eq!(
        index.links_from(CURIO_KP_ID).expect("query"),
        vec![(RUST_NOTES_KP_ID.to_owned(), "wikilink".to_owned())]
    );

    // Behavior folded from the events fixtures: 2 opens, star negated,
    // read-later kept, last activity = the newest event.
    let behavior = index
        .behavior(CURIO_KP_ID)
        .expect("query")
        .expect("events folded");
    assert_eq!(behavior.opened_count, 2);
    assert!(!behavior.starred, "negation honored");
    assert!(behavior.read_later);
    assert_eq!(
        behavior.last_activity.as_deref(),
        Some("2026-07-02T08:00:00.000Z")
    );
    index.close().expect("close");

    // ---- Second run: nothing changed → the expensive path MUST NOT run. ----
    let embedder2 = CountingEmbedder::new();
    let report2 = ingest(&cfg, &embedder2).expect("second ingest");
    assert_eq!(report2.unchanged, 5, "every indexed note is unchanged");
    assert_eq!(report2.ingested, 0);
    assert_eq!(
        embedder2.calls.load(Ordering::SeqCst),
        0,
        "unchanged notes must not re-embed"
    );
    assert_eq!(embedder2.texts.load(Ordering::SeqCst), 0);
    let events2 = report2.events.as_ref().expect("curio enabled");
    assert_eq!(events2.folded, 0, "cursors resume past all events");
    assert_eq!(events2.duplicates, 0);
}

#[test]
fn rebuild_reproduces_the_ingest_corpus() {
    let dir = tempfile::tempdir().expect("tempdir");
    let index_path = dir.path().join("index.db");
    let cfg = config(&index_path);
    let embedder = CountingEmbedder::new();
    ingest(&cfg, &embedder).expect("ingest");

    // Blue/green rebuild: SAME corpus and identities as ingest — the
    // .kpignore'd and schema-violating notes stay out, the Curio note
    // keeps its curio: identity, links and behavior are re-derived.
    let report = kp_ingest::rebuild(&cfg, &embedder).expect("rebuild");
    assert_eq!(report.epoch, 2, "epoch counter advances");
    assert_eq!(report.notes_indexed, 5);
    assert_eq!(report.notes_skipped, 1);
    assert_eq!(report.ignored, 2);
    assert_eq!(report.links, 6);
    let events = report.events.as_ref().expect("curio enabled");
    assert_eq!(
        events.folded, 6,
        "fresh epoch re-folds the retained event log"
    );

    let probe = HashEmbedder::new(64);
    let index = Index::open(&index_path, &probe).expect("open");
    assert_eq!(index.meta().epoch, 2);
    assert!(index.note_state(CURIO_KP_ID).expect("query").is_some());
    assert!(
        index
            .note_state("path:curio/rust-async-patterns.md")
            .expect("query")
            .is_none(),
        "the curio note must NOT reappear under a path identity"
    );
    assert!(
        index
            .note_state("path:drafts/wip.md")
            .expect("query")
            .is_none()
    );
    let behavior = index
        .behavior(CURIO_KP_ID)
        .expect("query")
        .expect("re-folded");
    assert_eq!(behavior.opened_count, 2);
    assert!(!behavior.starred);
    assert_eq!(
        index.links_from(CURIO_KP_ID).expect("query"),
        vec![(RUST_NOTES_KP_ID.to_owned(), "wikilink".to_owned())]
    );
}

/// Recursive fixture copy so a test can MUTATE its vault (the checked-in
/// fixture tree stays read-only).
fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("readdir") {
        let entry = entry.expect("entry");
        let dest = to.join(entry.file_name());
        if entry.file_type().expect("type").is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), &dest).expect("copy");
        }
    }
}

/// Regression (change-token bug): user enrichment OUTSIDE the Curio
/// managed region does not move the producer-declared checksum, but it
/// MUST still re-index — search, related() and the link graph all read
/// the whole note.
#[test]
fn user_edits_outside_the_managed_region_reindex() {
    let dir = tempfile::tempdir().expect("tempdir");
    let vault_root = dir.path().join("vault");
    copy_dir(
        &fixture("fixtures/vault"),
        &vault_root, // mutable copy
    );
    let index_path = dir.path().join("index.db");
    let toml = format!(
        "schema = \"kp-config/v1\"\n[vault]\npath = \"{}\"\n[index]\npath = \"{}\"\nembedder = \"hash\"\n",
        vault_root.display(),
        index_path.display(),
    );
    let cfg = KpConfig::from_toml_str(&toml).expect("parses");
    ingest(&cfg, &CountingEmbedder::new()).expect("first ingest");

    // Enrich BELOW the managed region: the declared frontmatter checksum
    // (managed-region bytes only) is untouched.
    let note_path = vault_root.join("curio/rust-async-patterns.md");
    let original = std::fs::read_to_string(&note_path).expect("read");
    std::fs::write(
        &note_path,
        format!("{original}\nAlso see [[guides/chunking|the chunking guide]].\n"),
    )
    .expect("append");

    let embedder = CountingEmbedder::new();
    let report = ingest(&cfg, &embedder).expect("second ingest");
    assert_eq!(
        report.ingested, 1,
        "the enriched note must re-index even though the declared checksum is unchanged"
    );
    assert_eq!(
        embedder.calls.load(Ordering::SeqCst),
        1,
        "the enriched note re-embeds"
    );

    // The new companion wikilink is visible to the link graph.
    let probe = HashEmbedder::new(64);
    let index = Index::open(&index_path, &probe).expect("open");
    let links = index.links_from(CURIO_KP_ID).expect("query");
    assert!(
        links.iter().any(|(to, _)| to == "path:guides/chunking.md"),
        "companion wikilink must be recorded, got {links:?}"
    );
    index.close().expect("close");

    // Same class of hole for kp-frontmatter notes carrying a stale
    // producer-declared checksum (the Zotero shape): body edits that do
    // not touch the declared value must still re-index.
    let zotero_like = vault_root.join("notes/stale-checksum.md");
    let declared = "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
    std::fs::write(
        &zotero_like,
        format!(
            "---\nkp_id: \"zotero:KEYSTALE\"\nkp_schema: kp-note/v1\ntitle: S\nchecksum: \"{declared}\"\n---\nmanaged text\n"
        ),
    )
    .expect("write");
    let report = ingest(&cfg, &CountingEmbedder::new()).expect("ingest new note");
    assert_eq!(report.ingested, 1);
    std::fs::write(
        &zotero_like,
        format!(
            "---\nkp_id: \"zotero:KEYSTALE\"\nkp_schema: kp-note/v1\ntitle: S\nchecksum: \"{declared}\"\n---\nmanaged text\n\nmy margin notes\n"
        ),
    )
    .expect("edit body, checksum untouched");
    let report = ingest(&cfg, &CountingEmbedder::new()).expect("re-ingest");
    assert_eq!(
        report.ingested, 1,
        "body edit with a stale declared checksum must re-index"
    );
}

#[test]
fn vanished_notes_are_pruned_and_edits_reembed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let vault_root = dir.path().join("vault");
    std::fs::create_dir_all(&vault_root).expect("mkdir");
    std::fs::write(vault_root.join("a.md"), "# A\n\nalpha body\n").expect("write");
    std::fs::write(vault_root.join("b.md"), "# B\n\nbeta body\n").expect("write");
    let index_path = dir.path().join("index.db");
    let toml = format!(
        "schema = \"kp-config/v1\"\n[vault]\npath = \"{}\"\n[index]\npath = \"{}\"\nembedder = \"hash\"\n",
        vault_root.display(),
        index_path.display(),
    );
    let cfg = KpConfig::from_toml_str(&toml).expect("parses");
    let embedder = CountingEmbedder::new();

    let report = ingest(&cfg, &embedder).expect("ingest");
    assert_eq!(report.ingested, 2);

    // Edit one note, delete the other.
    std::fs::write(vault_root.join("a.md"), "# A\n\nalpha body, edited\n").expect("write");
    std::fs::remove_file(vault_root.join("b.md")).expect("remove");

    let embedder2 = CountingEmbedder::new();
    let report = ingest(&cfg, &embedder2).expect("ingest");
    assert_eq!(report.ingested, 1, "the edited note re-embeds");
    assert_eq!(report.unchanged, 0);
    assert_eq!(report.removed, 1, "the vanished note is pruned");
    assert_eq!(embedder2.calls.load(Ordering::SeqCst), 1);

    let probe = HashEmbedder::new(64);
    let index = Index::open(&index_path, &probe).expect("open");
    assert!(index.note_state("path:b.md").expect("query").is_none());
    let a = index
        .note_state("path:a.md")
        .expect("query")
        .expect("still indexed");
    assert_eq!(a.path, "a.md");
}
