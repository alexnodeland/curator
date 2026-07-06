//! Mapping golden tests — one recorded item fixture per item type
//! (journalArticle, book, webpage, report + the generic fallback), each
//! pinned to the exact kp-note/v1 file the mapper renders.
//!
//! Regenerate deliberately with `KP_UPDATE_GOLDENS=1 cargo test -p
//! curator-zotero --test mapping`, then review the diff — the goldens are the
//! mapping contract.

use std::path::PathBuf;

use curator_zotero::item::Item;
use curator_zotero::map::map_item;

const PAGE1: &str = include_str!("../../../fixtures/zotero/items-page1.json");
const PAGE2: &str = include_str!("../../../fixtures/zotero/items-page2.json");
const REPORT: &str = include_str!("../../../fixtures/zotero/item-report.json");
const GENERIC: &str = include_str!("../../../fixtures/zotero/item-generic.json");
const FULLTEXT: &str = include_str!("../../../fixtures/zotero/fulltext.json");

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/zotero/golden")
}

fn check_golden(name: &str, rendered: &str) {
    let path = golden_dir().join(name);
    if std::env::var_os("KP_UPDATE_GOLDENS").is_some() {
        std::fs::write(&path, rendered).expect("write golden");
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing golden {}: {e}", path.display()));
    assert_eq!(
        rendered, expected,
        "golden {name} drifted — review, then KP_UPDATE_GOLDENS=1 to accept"
    );
}

fn items(fixture: &str) -> Vec<Item> {
    serde_json::from_str(fixture).expect("fixture parses")
}

fn item(fixture: &str) -> Item {
    serde_json::from_str(fixture).expect("fixture parses")
}

#[test]
fn golden_journal_article_with_attachment_and_fulltext() {
    let page1 = items(PAGE1);
    let page2 = items(PAGE2);
    let article = &page1[0];
    let attachment = &page2[1];
    let fulltext: curator_zotero::item::Fulltext =
        serde_json::from_str(FULLTEXT).expect("fulltext fixture");

    let mapped = map_item(
        article,
        std::slice::from_ref(attachment),
        Some(&fulltext.content),
        20_000,
    );
    assert_eq!(mapped.frontmatter.kp_id.to_string(), "zotero:KEYJRNL1");
    check_golden("journal-article.md", &mapped.fresh_content());
}

#[test]
fn golden_book() {
    let page1 = items(PAGE1);
    let mapped = map_item(&page1[1], &[], None, 20_000);
    assert_eq!(mapped.frontmatter.kp_id.to_string(), "zotero:KEYBOOK1");
    // No publication date beyond a year — anchored to Jan 1.
    assert_eq!(
        mapped.frontmatter.created.as_deref(),
        Some("1984-01-01T00:00:00Z")
    );
    check_golden("book.md", &mapped.fresh_content());
}

#[test]
fn golden_webpage() {
    let page2 = items(PAGE2);
    let mapped = map_item(&page2[0], &[], None, 20_000);
    assert_eq!(mapped.frontmatter.kp_id.to_string(), "zotero:KEYWEBP1");
    assert_eq!(
        mapped.frontmatter.source.as_deref(),
        Some("https://durable.example.test/plain-text")
    );
    check_golden("webpage.md", &mapped.fresh_content());
}

#[test]
fn golden_report() {
    let mapped = map_item(&item(REPORT), &[], None, 20_000);
    assert_eq!(mapped.frontmatter.kp_id.to_string(), "zotero:KEYRPRT1");
    check_golden("report.md", &mapped.fresh_content());
}

#[test]
fn golden_generic_fallback() {
    let mapped = map_item(&item(GENERIC), &[], None, 20_000);
    assert_eq!(mapped.frontmatter.kp_id.to_string(), "zotero:KEYPODC1");
    // No date and no dateAdded fallback needed: dateAdded present.
    assert_eq!(
        mapped.frontmatter.created.as_deref(),
        Some("2026-06-27T20:00:00Z")
    );
    check_golden("generic-podcast.md", &mapped.fresh_content());
}

#[test]
fn rendered_notes_reingest_cleanly_as_kp_notes() {
    // Every golden is a valid kp-note/v1 file whose declared checksum
    // covers exactly the managed region.
    use curator_core::Note;
    use curator_core::note::Frontmatter;
    use curator_zotero::managed::split_managed;

    for fixture in [PAGE1, PAGE2] {
        for it in items(fixture).iter().filter(|i| !i.is_attachment()) {
            let mapped = map_item(it, &[], None, 20_000);
            let content = mapped.fresh_content();
            let note = Note::parse("zotero/x.md", &content).expect("rendered note parses");
            let Frontmatter::Kp(fm) = &note.frontmatter else {
                panic!("rendered note must carry the KP block");
            };
            assert_eq!(fm.kp_schema, "kp-note/v1");
            assert_eq!(fm.kp_id.to_string(), format!("zotero:{}", it.key()));
            let split = split_managed(&note.body).expect("markers present");
            assert_eq!(
                fm.checksum.as_ref().expect("checksum present"),
                &curator_core::Checksum::compute(split.managed.as_bytes()),
                "checksum must cover exactly the managed region"
            );
        }
    }
}

#[test]
fn fulltext_truncation_lands_in_the_note() {
    let page1 = items(PAGE1);
    let long = "word ".repeat(100);
    let mapped = map_item(&page1[0], &[], Some(&long), 40);
    assert!(
        mapped
            .managed
            .contains("*[fulltext truncated at 40 characters]*"),
        "{}",
        mapped.managed
    );
    // The kept prefix is at most 40 chars.
    let ft = mapped.managed.split("## Fulltext\n\n").nth(1).expect("ft");
    let kept = ft.split("\n\n*[fulltext").next().expect("kept");
    assert!(kept.chars().count() <= 40, "{kept:?}");
}
