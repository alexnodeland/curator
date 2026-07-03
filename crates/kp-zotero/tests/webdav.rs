//! Channel 2 fallback — the WebDAV `.prop`/`.zip` shim: real-shaped
//! `.prop` XML fixture, a tiny zip built in-test, CRC pass/fail, size
//! caps, and the wiremock-served end-to-end path.

use std::io::Write;

use kp_zotero::error::ZoteroError;
use kp_zotero::webdav::{PropInfo, ShimCaps, WebDavShim, extract_fulltext, parse_prop};
use tokio::runtime::Runtime;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const PROP_FIXTURE: &str = include_str!("../../../fixtures/zotero/storage.prop");

const HTML_BODY: &str = "<html><body><h1>Analytical Engines</h1>\
                         <p>Programme cards &amp; loops.</p></body></html>";

/// Build a `<KEY>.zip` the way Zotero's WebDAV client does (one payload
/// entry), Stored so tests can corrupt payload bytes in place.
fn zip_with(name: &str, content: &[u8], method: CompressionMethod) -> Vec<u8> {
    let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file(
            name,
            SimpleFileOptions::default().compression_method(method),
        )
        .expect("start file");
    writer.write_all(content).expect("write entry");
    writer.finish().expect("finish").into_inner()
}

fn crc_prop(content: &[u8]) -> PropInfo {
    PropInfo {
        version: Some("1".to_owned()),
        mtime: Some(1_767_103_260_000),
        hash: Some(format!("{:08x}", crc32fast::hash(content))),
    }
}

#[test]
fn parses_the_real_shaped_prop_fixture() {
    let prop = parse_prop(PROP_FIXTURE).expect("parses");
    assert_eq!(prop.version.as_deref(), Some("1"));
    assert_eq!(prop.mtime, Some(1_767_103_260_000));
    // zotero.org's client writes an MD5 here (32 hex) — carried through.
    assert_eq!(
        prop.hash.as_deref(),
        Some("0123456789abcdef0123456789abcdef")
    );
}

#[test]
fn malformed_prop_is_a_typed_error() {
    let err = parse_prop("<properties version=\"1\"><mtime>1<").unwrap_err();
    assert!(matches!(err, ZoteroError::Prop(_)), "got {err:?}");
}

#[test]
fn extracts_html_entry_and_verifies_crc() {
    let bytes = zip_with(
        "KEYATT01.html",
        HTML_BODY.as_bytes(),
        CompressionMethod::Stored,
    );
    let text = extract_fulltext(
        &bytes,
        &crc_prop(HTML_BODY.as_bytes()),
        "KEYATT01",
        ShimCaps::default(),
    )
    .expect("extracts");
    assert_eq!(text, "Analytical Engines\n\nProgramme cards & loops.");
}

#[test]
fn extracts_txt_entry_when_no_html_present() {
    let bytes = zip_with(
        "KEYATT01.txt",
        b"plain text payload",
        CompressionMethod::Deflated,
    );
    let text = extract_fulltext(
        &bytes,
        &crc_prop(b"plain text payload"),
        "KEYATT01",
        ShimCaps::default(),
    )
    .expect("extracts");
    assert_eq!(text, "plain text payload");
}

#[test]
fn md5_shaped_prop_hash_is_carried_not_verified() {
    let bytes = zip_with("K.txt", b"payload", CompressionMethod::Stored);
    let prop = parse_prop(PROP_FIXTURE).expect("parses");
    let text = extract_fulltext(&bytes, &prop, "K", ShimCaps::default()).expect("extracts");
    assert_eq!(text, "payload");
}

#[test]
fn corrupted_entry_bytes_fail_the_crc_check() {
    let content = b"the quick brown fox jumps over the lazy dog";
    let mut bytes = zip_with("K.txt", content, CompressionMethod::Stored);
    // Stored entries embed the payload verbatim — flip one byte of it.
    let at = bytes
        .windows(content.len())
        .position(|w| w == content)
        .expect("stored payload present");
    bytes[at] ^= 0xFF;
    let err = extract_fulltext(&bytes, &crc_prop(content), "K", ShimCaps::default()).unwrap_err();
    assert!(
        matches!(
            err,
            ZoteroError::ZipRead(_) | ZoteroError::CrcMismatch { .. }
        ),
        "got {err:?}"
    );
}

#[test]
fn prop_declared_crc_mismatch_fails() {
    let bytes = zip_with("K.txt", b"payload", CompressionMethod::Stored);
    let prop = PropInfo {
        version: Some("1".to_owned()),
        mtime: None,
        hash: Some("00000000".to_owned()),
    };
    let err = extract_fulltext(&bytes, &prop, "K", ShimCaps::default()).unwrap_err();
    assert!(
        matches!(err, ZoteroError::CrcMismatch { .. }),
        "got {err:?}"
    );
}

#[test]
fn oversized_entry_is_refused() {
    let big = vec![b'x'; 4096];
    let bytes = zip_with("K.txt", &big, CompressionMethod::Stored);
    let caps = ShimCaps {
        max_zip_bytes: 1024 * 1024,
        max_entry_bytes: 1024,
    };
    let err = extract_fulltext(&bytes, &crc_prop(&big), "K", caps).unwrap_err();
    assert!(
        matches!(
            err,
            ZoteroError::TooLarge {
                what: "zip entry",
                ..
            }
        ),
        "got {err:?}"
    );
}

#[test]
fn zip_without_a_text_entry_is_an_error() {
    let bytes = zip_with("K.pdf", b"%PDF-1.7 ...", CompressionMethod::Stored);
    let err = extract_fulltext(&bytes, &PropInfo::default(), "K", ShimCaps::default()).unwrap_err();
    assert!(matches!(err, ZoteroError::Zip(_)), "got {err:?}");
}

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
}

#[test]
fn end_to_end_prop_then_zip_over_webdav() {
    let s = Server::start();
    let content = HTML_BODY.as_bytes();
    let prop_xml = format!(
        "<properties version=\"1\">\n  <mtime>1767103260000</mtime>\n  <hash>{:08x}</hash>\n</properties>\n",
        crc32fast::hash(content)
    );
    let zip_bytes = zip_with("KEYATT01.html", content, CompressionMethod::Deflated);
    s.mount(
        Mock::given(method("GET"))
            .and(path("/dav/KEYATT01.prop"))
            .respond_with(ResponseTemplate::new(200).set_body_string(prop_xml))
            .expect(1),
    );
    s.mount(
        Mock::given(method("GET"))
            .and(path("/dav/KEYATT01.zip"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(zip_bytes))
            .expect(1),
    );

    let http = reqwest::blocking::Client::new();
    let base = format!("{}/dav", s.server.uri());
    let shim = WebDavShim::new(&http, &base, ShimCaps::default());
    let text = shim
        .fetch_fulltext("KEYATT01")
        .expect("fetch")
        .expect("some");
    assert_eq!(text, "Analytical Engines\n\nProgramme cards & loops.");
}

#[test]
fn missing_prop_means_no_webdav_store_not_an_error() {
    let s = Server::start();
    s.mount(
        Mock::given(method("GET"))
            .and(path("/dav/KEYNONE1.prop"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1),
    );
    let http = reqwest::blocking::Client::new();
    let base = format!("{}/dav", s.server.uri());
    let shim = WebDavShim::new(&http, &base, ShimCaps::default());
    assert!(shim.fetch_fulltext("KEYNONE1").expect("fetch").is_none());
}

#[test]
fn oversized_zip_download_is_refused() {
    let s = Server::start();
    s.mount(
        Mock::given(method("GET"))
            .and(path("/dav/KEYBIG01.prop"))
            .respond_with(ResponseTemplate::new(200).set_body_string(PROP_FIXTURE)),
    );
    s.mount(
        Mock::given(method("GET"))
            .and(path("/dav/KEYBIG01.zip"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 4096])),
    );
    let http = reqwest::blocking::Client::new();
    let base = format!("{}/dav", s.server.uri());
    let shim = WebDavShim::new(
        &http,
        &base,
        ShimCaps {
            max_zip_bytes: 1024,
            max_entry_bytes: 1024,
        },
    );
    let err = shim.fetch_fulltext("KEYBIG01").unwrap_err();
    assert!(
        matches!(err, ZoteroError::TooLarge { what: "zip", .. }),
        "got {err:?}"
    );
}
