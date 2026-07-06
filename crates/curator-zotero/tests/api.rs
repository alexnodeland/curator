//! Channel 1 — wiremock-driven Web API sequences over recorded fixtures:
//! initial sync with pagination, delta with 304, tombstones, fulltext,
//! error surfacing. Hermetic: every request hits the local mock server.

use curator_zotero::api::{ZoteroApi, parse_link_next};
use curator_zotero::error::ZoteroError;
use tokio::runtime::Runtime;
use wiremock::matchers::{header, method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PAGE1: &str = include_str!("../../../fixtures/zotero/items-page1.json");
const PAGE2: &str = include_str!("../../../fixtures/zotero/items-page2.json");
const DELTA: &str = include_str!("../../../fixtures/zotero/items-delta.json");
const DELETED: &str = include_str!("../../../fixtures/zotero/deleted.json");
const FULLTEXT: &str = include_str!("../../../fixtures/zotero/fulltext.json");

/// A mock server + the runtime that keeps it serving. The blocking client
/// under test runs on the test thread; wiremock runs on the runtime's
/// workers.
struct Server {
    rt: Runtime,
    server: MockServer,
}

impl Server {
    fn start() -> Self {
        let rt = Runtime::new().expect("runtime");
        let server = rt.block_on(MockServer::start());
        Self { rt, server }
    }

    fn mount(&self, mock: Mock) {
        self.rt.block_on(mock.mount(&self.server));
    }

    fn uri(&self) -> String {
        self.server.uri()
    }

    fn api(&self) -> ZoteroApi {
        ZoteroApi::new(&self.uri(), "777", "test-key")
    }
}

fn json_response(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "application/json")
}

#[test]
fn initial_sync_paginates_and_captures_the_version() {
    let s = Server::start();
    let next = format!(
        "<{}/users/777/items?format=json&limit=100&since=0&start=2>; rel=\"next\"",
        s.uri()
    );
    s.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items"))
            .and(query_param("since", "0"))
            .and(query_param("limit", "100"))
            .and(query_param("format", "json"))
            .and(query_param_is_missing("start"))
            .and(header("Zotero-API-Version", "3"))
            .and(header("Zotero-API-Key", "test-key"))
            .respond_with(
                json_response(PAGE1)
                    .insert_header("Last-Modified-Version", "42")
                    .insert_header("Link", next.as_str()),
            )
            .expect(1),
    );
    s.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items"))
            .and(query_param("start", "2"))
            .respond_with(json_response(PAGE2).insert_header("Last-Modified-Version", "42"))
            .expect(1),
    );

    let delta = s.api().items_since(None).expect("initial sync");
    assert!(!delta.not_modified);
    assert_eq!(delta.version, 42);
    let keys: Vec<&str> = delta.items.iter().map(|i| i.key()).collect();
    assert_eq!(keys, vec!["KEYJRNL1", "KEYBOOK1", "KEYWEBP1", "KEYATT01"]);
    // Typed payloads survived the ride.
    assert_eq!(delta.items[0].data.item_type, "journalArticle");
    assert_eq!(delta.items[0].data.creators.len(), 2);
    assert!(delta.items[3].is_attachment());
    assert_eq!(delta.items[3].data.parent_item, "KEYJRNL1");
}

#[test]
fn unchanged_library_costs_one_304() {
    let s = Server::start();
    s.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items"))
            .and(query_param("since", "42"))
            .and(header("If-Modified-Since-Version", "42"))
            .respond_with(ResponseTemplate::new(304))
            .expect(1),
    );

    let delta = s.api().items_since(Some(42)).expect("delta");
    assert!(delta.not_modified);
    assert_eq!(delta.version, 42);
    assert!(delta.items.is_empty());
}

#[test]
fn delta_returns_only_changed_items_and_the_new_version() {
    let s = Server::start();
    s.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items"))
            .and(query_param("since", "42"))
            .and(header("If-Modified-Since-Version", "42"))
            .respond_with(json_response(DELTA).insert_header("Last-Modified-Version", "45"))
            .expect(1),
    );

    let delta = s.api().items_since(Some(42)).expect("delta");
    assert!(!delta.not_modified);
    assert_eq!(delta.version, 45);
    assert_eq!(delta.items.len(), 1);
    assert_eq!(delta.items[0].key(), "KEYBOOK1");
    assert_eq!(delta.items[0].version, 45);
}

#[test]
fn deleted_since_returns_tombstoned_item_keys() {
    let s = Server::start();
    s.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/deleted"))
            .and(query_param("since", "42"))
            .respond_with(json_response(DELETED))
            .expect(1),
    );

    let deleted = s.api().deleted_since(42).expect("tombstones");
    assert_eq!(deleted.items, vec!["KEYJRNL1", "KEYWEBP1"]);
}

#[test]
fn fulltext_present_and_absent() {
    let s = Server::start();
    s.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items/KEYATT01/fulltext"))
            .respond_with(json_response(FULLTEXT))
            .expect(1),
    );
    s.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items/KEYNONE1/fulltext"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1),
    );

    let api = s.api();
    let ft = api.fulltext("KEYATT01").expect("fulltext").expect("some");
    assert!(ft.content.starts_with("Analytical Engines Reconsidered."));
    assert_eq!(ft.indexed_pages, Some(15));
    assert_eq!(ft.total_pages, Some(15));
    assert!(api.fulltext("KEYNONE1").expect("fulltext").is_none());
}

#[test]
fn non_success_statuses_surface_as_typed_errors() {
    let s = Server::start();
    s.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items"))
            .respond_with(ResponseTemplate::new(500)),
    );

    let err = s.api().items_since(None).unwrap_err();
    assert!(
        matches!(err, ZoteroError::Status { status: 500, .. }),
        "got {err:?}"
    );
}

#[test]
fn a_200_without_the_version_header_is_an_error() {
    let s = Server::start();
    s.mount(
        Mock::given(method("GET"))
            .and(path("/users/777/items"))
            .respond_with(json_response("[]")),
    );

    let err = s.api().items_since(None).unwrap_err();
    assert!(matches!(err, ZoteroError::MissingVersion), "got {err:?}");
}

#[test]
fn link_next_parsing_matches_the_api_shape() {
    // The exact header shape zotero.org sends.
    let raw = "<https://api.example.test/users/777/items?limit=100&start=100>; rel=\"next\", \
               <https://api.example.test/users/777/items?limit=100&start=1400>; rel=\"last\", \
               <https://www.example.test/users/777/items>; rel=\"alternate\"";
    assert_eq!(
        parse_link_next(raw).as_deref(),
        Some("https://api.example.test/users/777/items?limit=100&start=100")
    );
}
