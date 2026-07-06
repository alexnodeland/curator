//! `curator zotero sync` orchestration, end to end: wiremock API + temp vault +
//! hash-embedder index. Covers initial sync, 304 delta, user-edit
//! preservation, trash-not-delete tombstones, the orphan-attachment path,
//! the WebDAV fallback, and clean disablement.

use std::io::Write;

use curator_index::{HashEmbedder, Index};
use curator_zotero::sync::{CURSOR_CONSUMER, CURSOR_FILE, SyncOptions, sync};
use curator_zotero::{MANAGED_END, ZoteroError};
use tokio::runtime::Runtime;
use wiremock::matchers::{header, method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

const PAGE1: &str = include_str!("../../../fixtures/zotero/items-page1.json");
const PAGE2: &str = include_str!("../../../fixtures/zotero/items-page2.json");
const DELTA: &str = include_str!("../../../fixtures/zotero/items-delta.json");
const DELETED: &str = include_str!("../../../fixtures/zotero/deleted.json");
const FULLTEXT: &str = include_str!("../../../fixtures/zotero/fulltext.json");
const CHILDREN_JRNL: &str = include_str!("../../../fixtures/zotero/children-KEYJRNL1.json");
const ITEM_JRNL: &str = include_str!("../../../fixtures/zotero/item-KEYJRNL1.json");

struct Harness {
    rt: Runtime,
    server: MockServer,
    _dir: tempfile::TempDir,
    vault_root: std::path::PathBuf,
    index: Index,
    key_env: String,
}

impl Harness {
    fn start(key_env: &str) -> Self {
        let rt = Runtime::new().expect("runtime");
        let server = rt.block_on(MockServer::start());
        let dir = tempfile::tempdir().expect("tempdir");
        let vault_root = dir.path().join("vault");
        std::fs::create_dir_all(&vault_root).expect("mkdir");
        let index =
            Index::create(dir.path().join("index.db"), &HashEmbedder::default(), 1).expect("index");
        Self {
            rt,
            server,
            _dir: dir,
            vault_root,
            index,
            key_env: key_env.to_owned(),
        }
    }

    fn mount(&self, mock: Mock) {
        self.rt.block_on(mock.mount(&self.server));
    }

    fn reset(&self) {
        self.rt.block_on(self.server.reset());
    }

    fn config(&self, extra: &str) -> curator_core::KpConfig {
        let toml = format!(
            "schema = \"kp-config/v1\"\n\
             [vault]\npath = \"{}\"\n\
             [zotero]\nenabled = true\napi_base = \"{}\"\nuser_id = \"777\"\napi_key_env = \"{}\"\n{extra}",
            self.vault_root.display(),
            self.server.uri(),
            self.key_env,
        );
        curator_core::KpConfig::from_toml_str(&toml).expect("config parses")
    }

    fn run(&mut self, extra_config: &str) -> Result<curator_zotero::SyncReport, ZoteroError> {
        let config = self.config(extra_config);
        let options = SyncOptions::default();
        temp_env::with_var(&self.key_env, Some("test-key"), || {
            sync(&config, &mut self.index, &options)
        })
    }

    fn note(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.vault_root.join(rel)).ok()
    }
}

fn json(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "application/json")
}

fn empty_children() -> ResponseTemplate {
    json("[]")
}

/// Mount the full initial-sync surface: paginated items, children,
/// fulltext for the article's attachment (404 elsewhere).
fn mount_initial(h: &Harness) {
    let next = format!(
        "<{}/users/777/items?format=json&limit=100&since=0&start=2>; rel=\"next\"",
        h.server.uri()
    );
    h.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items"))
            .and(query_param("since", "0"))
            .and(query_param_is_missing("start"))
            .and(header("Zotero-API-Key", "test-key"))
            .respond_with(
                json(PAGE1)
                    .insert_header("Last-Modified-Version", "42")
                    .insert_header("Link", next.as_str()),
            )
            .expect(1),
    );
    h.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items"))
            .and(query_param("start", "2"))
            .respond_with(json(PAGE2).insert_header("Last-Modified-Version", "42"))
            .expect(1),
    );
    h.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items/KEYJRNL1/children"))
            .respond_with(json(CHILDREN_JRNL)),
    );
    for key in ["KEYBOOK1", "KEYWEBP1"] {
        h.mount(
            Mock::given(method("GET"))
                .and(path(format!("/users/777/items/{key}/children")))
                .respond_with(empty_children()),
        );
    }
    h.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items/KEYATT01/fulltext"))
            .respond_with(json(FULLTEXT)),
    );
}

#[test]
fn initial_sync_writes_notes_and_persists_the_cursor() {
    let mut h = Harness::start("KP_ZOT_TEST_INITIAL");
    mount_initial(&h);

    let report = h.run("").expect("sync");
    assert!(report.enabled);
    assert!(!report.not_modified);
    assert_eq!(report.version_before, None);
    assert_eq!(report.version_after, Some(42));
    assert_eq!(report.fetched, 4);
    assert_eq!(report.upserted, 3, "warnings: {:?}", report.warnings);
    assert_eq!(report.unchanged, 0);
    assert_eq!(report.fulltext_added, 1);
    assert_eq!(report.fulltext_missing, 2);
    assert_eq!(report.tombstones, 0);

    // The three notes landed under the configured dir, as kp-note/v1.
    let article = h.note("zotero/KEYJRNL1.md").expect("article note");
    assert!(article.contains("kp_id: zotero:KEYJRNL1"));
    assert!(article.contains("kp_schema: kp-note/v1"));
    assert!(article.contains("## Fulltext"));
    assert!(article.contains("programme cards"));
    assert!(h.note("zotero/KEYBOOK1.md").is_some());
    assert!(h.note("zotero/KEYWEBP1.md").is_some());
    // The Zotero child note never became a file.
    assert!(h.note("zotero/KEYNOTE1.md").is_none());

    // The cursor persisted in curator-index.
    assert_eq!(
        h.index
            .cursor(CURSOR_CONSUMER, CURSOR_FILE)
            .expect("cursor"),
        Some(42)
    );
}

#[test]
fn unchanged_library_is_one_304_and_no_writes() {
    let mut h = Harness::start("KP_ZOT_TEST_304");
    mount_initial(&h);
    h.run("").expect("initial sync");
    h.reset();

    h.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items"))
            .and(query_param("since", "42"))
            .and(header("If-Modified-Since-Version", "42"))
            .respond_with(ResponseTemplate::new(304))
            .expect(1),
    );
    let report = h.run("").expect("second sync");
    assert!(report.not_modified);
    assert_eq!(report.upserted, 0);
    assert_eq!(report.version_after, Some(42));
    assert_eq!(
        h.index
            .cursor(CURSOR_CONSUMER, CURSOR_FILE)
            .expect("cursor"),
        Some(42)
    );
}

#[test]
fn delta_updates_preserve_user_edits_and_tombstones_trash_not_delete() {
    let mut h = Harness::start("KP_ZOT_TEST_DELTA");
    mount_initial(&h);
    h.run("").expect("initial sync");
    h.reset();

    // The user annotates two notes: the book below the managed region
    // (it will be UPDATED by the delta) and the article (it will be
    // TOMBSTONED — must go to trash, not oblivion).
    for rel in ["zotero/KEYBOOK1.md", "zotero/KEYJRNL1.md"] {
        let p = h.vault_root.join(rel);
        let raw = std::fs::read_to_string(&p).expect("read");
        std::fs::write(
            &p,
            raw.replace(
                &format!("{MANAGED_END}\n"),
                &format!("{MANAGED_END}\n\nMy own careful annotations.\n"),
            ),
        )
        .expect("write");
    }

    // Delta: book changed (v45); article + webpage tombstoned.
    h.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items"))
            .and(query_param("since", "42"))
            .and(header("If-Modified-Since-Version", "42"))
            .respond_with(json(DELTA).insert_header("Last-Modified-Version", "45"))
            .expect(1),
    );
    h.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items/KEYBOOK1/children"))
            .respond_with(empty_children()),
    );
    h.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/deleted"))
            .and(query_param("since", "42"))
            .respond_with(json(DELETED))
            .expect(1),
    );

    let report = h.run("").expect("delta sync");
    assert_eq!(report.upserted, 1);
    assert_eq!(report.tombstones, 2);
    assert_eq!(report.deleted_files, 1, "webpage was pristine");
    assert_eq!(report.trashed_files, 1, "article was user-edited");
    assert_eq!(report.version_after, Some(45));

    // Book: machine content refreshed, user zone intact.
    let book = h.note("zotero/KEYBOOK1.md").expect("book note");
    assert!(book.contains("Second Edition"), "{book}");
    assert!(book.contains("My own careful annotations."), "{book}");

    // Webpage (pristine): gone, and NOT in trash.
    assert!(h.note("zotero/KEYWEBP1.md").is_none());
    assert!(h.note(".kp/trash/KEYWEBP1.md").is_none());

    // Article (user-edited): moved to trash with the annotations.
    assert!(h.note("zotero/KEYJRNL1.md").is_none());
    let trashed = h.note(".kp/trash/KEYJRNL1.md").expect("trashed copy");
    assert!(trashed.contains("My own careful annotations."));

    assert_eq!(
        h.index
            .cursor(CURSOR_CONSUMER, CURSOR_FILE)
            .expect("cursor"),
        Some(45)
    );
}

#[test]
fn changed_attachment_refreshes_its_absent_parent() {
    let mut h = Harness::start("KP_ZOT_TEST_ORPHAN");
    // Cursor exists; the delta contains ONLY the attachment.
    h.index
        .set_cursor(CURSOR_CONSUMER, CURSOR_FILE, 42)
        .expect("cursor");
    let attachment_only = {
        let page2: serde_json::Value = serde_json::from_str(PAGE2).expect("json");
        serde_json::to_string(&serde_json::Value::Array(vec![page2[1].clone()])).expect("json")
    };
    h.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items"))
            .and(query_param("since", "42"))
            .respond_with(json(&attachment_only).insert_header("Last-Modified-Version", "43"))
            .expect(1),
    );
    // The parent is fetched individually, then its children + fulltext.
    h.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items/KEYJRNL1"))
            .respond_with(json(ITEM_JRNL))
            .expect(1),
    );
    h.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items/KEYJRNL1/children"))
            .respond_with(json(CHILDREN_JRNL)),
    );
    h.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items/KEYATT01/fulltext"))
            .respond_with(json(FULLTEXT)),
    );
    h.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/deleted"))
            .respond_with(json("{\"items\": []}")),
    );

    let report = h.run("").expect("sync");
    assert_eq!(report.upserted, 1, "warnings: {:?}", report.warnings);
    let article = h.note("zotero/KEYJRNL1.md").expect("parent re-rendered");
    assert!(article.contains("analytical-engines.pdf"));
}

#[test]
fn webdav_fallback_supplies_fulltext_when_the_api_has_none() {
    let mut h = Harness::start("KP_ZOT_TEST_WEBDAV");
    // Single item + attachment; official fulltext 404s; WebDAV serves it.
    let next_free = {
        let page1: serde_json::Value = serde_json::from_str(PAGE1).expect("json");
        serde_json::to_string(&serde_json::Value::Array(vec![page1[0].clone()])).expect("json")
    };
    h.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items"))
            .and(query_param("since", "0"))
            .respond_with(json(&next_free).insert_header("Last-Modified-Version", "50"))
            .expect(1),
    );
    h.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items/KEYJRNL1/children"))
            .respond_with(json(CHILDREN_JRNL)),
    );
    h.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items/KEYATT01/fulltext"))
            .respond_with(ResponseTemplate::new(404)),
    );
    let content = b"<html><body><p>Recovered from the WebDAV store.</p></body></html>";
    let zip_bytes = {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        w.start_file(
            "KEYATT01.html",
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )
        .expect("start");
        w.write_all(content).expect("write");
        w.finish().expect("finish").into_inner()
    };
    let prop = format!(
        "<properties version=\"1\"><mtime>1</mtime><hash>{:08x}</hash></properties>",
        crc32fast::hash(content)
    );
    h.mount(
        Mock::given(method("GET"))
            .and(path("/dav/KEYATT01.prop"))
            .respond_with(ResponseTemplate::new(200).set_body_string(prop))
            .expect(1),
    );
    h.mount(
        Mock::given(method("GET"))
            .and(path("/dav/KEYATT01.zip"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(zip_bytes))
            .expect(1),
    );

    let extra = format!(
        "webdav_fallback = true\nwebdav_url = \"{}/dav\"\n",
        h.server.uri()
    );
    let report = h.run(&extra).expect("sync");
    assert_eq!(report.fulltext_added, 1, "warnings: {:?}", report.warnings);
    let article = h.note("zotero/KEYJRNL1.md").expect("note");
    assert!(
        article.contains("Recovered from the WebDAV store."),
        "{article}"
    );
}

#[test]
fn disabled_configurations_return_clean_reports_without_network() {
    // No mock server needed: a disabled sync must never touch the network.
    let dir = tempfile::tempdir().expect("tempdir");
    let vault_root = dir.path().join("vault");
    std::fs::create_dir_all(&vault_root).expect("mkdir");
    let mut index =
        Index::create(dir.path().join("index.db"), &HashEmbedder::default(), 1).expect("index");
    let options = SyncOptions::default();

    // enabled = false.
    let toml = format!(
        "schema = \"kp-config/v1\"\n[vault]\npath = \"{}\"\n[zotero]\nenabled = false\n",
        vault_root.display()
    );
    let config = curator_core::KpConfig::from_toml_str(&toml).expect("config");
    let report = sync(&config, &mut index, &options).expect("sync");
    assert!(!report.enabled);
    assert!(
        report
            .disabled_reason
            .as_deref()
            .unwrap()
            .contains("enabled")
    );

    // enabled = true but the key env is unset: cleanly disabled.
    let toml = format!(
        "schema = \"kp-config/v1\"\n[vault]\npath = \"{}\"\n\
         [zotero]\nenabled = true\nuser_id = \"777\"\napi_key_env = \"KP_ZOT_TEST_UNSET_XYZZY\"\n",
        vault_root.display()
    );
    let config = curator_core::KpConfig::from_toml_str(&toml).expect("config");
    let report = sync(&config, &mut index, &options).expect("sync");
    assert!(!report.enabled);
    assert!(
        report
            .disabled_reason
            .as_deref()
            .unwrap()
            .contains("KP_ZOT_TEST_UNSET_XYZZY")
    );

    // Key present but user_id empty: cleanly disabled.
    let toml = format!(
        "schema = \"kp-config/v1\"\n[vault]\npath = \"{}\"\n\
         [zotero]\nenabled = true\napi_key_env = \"KP_ZOT_TEST_SET_KEY\"\n",
        vault_root.display()
    );
    let config = curator_core::KpConfig::from_toml_str(&toml).expect("config");
    let report = temp_env::with_var("KP_ZOT_TEST_SET_KEY", Some("k"), || {
        sync(&config, &mut index, &options).expect("sync")
    });
    assert!(!report.enabled);
    assert!(
        report
            .disabled_reason
            .as_deref()
            .unwrap()
            .contains("user_id")
    );
}

#[test]
fn resync_of_an_unchanged_item_is_reported_unchanged() {
    let mut h = Harness::start("KP_ZOT_TEST_RESYNC");
    mount_initial(&h);
    h.run("").expect("initial");
    h.reset();

    // Simulate a lost cursor (fresh index) replaying the same delta: the
    // rendered bytes are identical, so files are untouched.
    h.index
        .remove_cursor(CURSOR_CONSUMER, CURSOR_FILE)
        .expect("drop cursor");
    mount_initial(&h);
    let report = h.run("").expect("resync");
    assert_eq!(report.upserted, 0);
    assert_eq!(report.unchanged, 3);
}
