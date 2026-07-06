//! The web-clip adapter — a producer seam for read-and-save browser tools.
//!
//! A browser **Web Viewer** (and clippers like it) turn an arbitrary web
//! page into a markdown note with YAML frontmatter — a source URL, title,
//! author, date, tags. That note lands in the vault as `Frontmatter::Foreign`
//! (some other tool's metadata): today the index sees only its filename and
//! body, losing the source, tags, and date. This adapter recognizes such a
//! note and maps its frontmatter onto `kp-note/v1`, so **ad-hoc web reading
//! becomes a first-class source** alongside Curio — searchable, citable
//! (source URL), tag-filterable, and recency-ranked (date).
//!
//! Two rules keep it safe:
//!
//! 1. **Tolerant, never a pinned contract.** Unlike Curio's vendored schema,
//!    a clipper's keys vary (users edit the template), so we *alias* the
//!    common ones and never fail — a note that isn't a recognizable web clip
//!    simply returns `None`. The defining, low-false-positive signal is an
//!    `http(s)` source/url the clip came from.
//! 2. **The vault file is NEVER rewritten.** Like the Curio adapter, this
//!    only synthesizes the in-memory indexable view; the producing tool keeps
//!    byte-ownership of its own frontmatter (the `Foreign` block is untouched
//!    on disk). Identity stays `path:<relpath>` — no new `kp_id` namespace,
//!    so nothing about the sha-pinned `kp-note/v1` contract changes.

use curator_core::{Frontmatter, KpId, Note, NoteFrontmatter};
use serde_yaml::Value;

/// Frontmatter keys accepted for each field, in priority order.
const SOURCE_KEYS: &[&str] = &["source", "url"];
const TITLE_KEYS: &[&str] = &["title"];
const CREATED_KEYS: &[&str] = &["published", "created", "date"];
const UPDATED_KEYS: &[&str] = &["updated", "modified"];
const TAG_KEYS: &[&str] = &["tags"];

/// The Curio producer's schema marker — its own adapter owns those notes, so
/// we must never double-claim one.
const CURIO_SCHEMA: &str = "curio.frontmatter.v1";

/// Recognize a browser-saved web clip and synthesize its `kp-note/v1` view,
/// or `None` when the note is not a web clip. Pure — no IO, no rewrite.
#[must_use]
pub fn adapt_webclip(note: &Note) -> Option<Note> {
    // Only foreign frontmatter is a candidate: kp-notes carry `kp_id`
    // (handled directly), Curio notes carry their schema (handled there).
    let Frontmatter::Foreign(yaml) = &note.frontmatter else {
        return None;
    };
    let map: Value = serde_yaml::from_str(yaml).ok()?;
    if map.get("schema").and_then(Value::as_str) == Some(CURIO_SCHEMA) {
        return None;
    }

    // The defining signal: an http(s) URL the clip came from.
    let source = first_url(&map, SOURCE_KEYS)?;

    // Title: the clip's own, else the file stem (`Note::title`).
    let title = first_str(&map, TITLE_KEYS).unwrap_or_else(|| note.title());

    let mut fm = NoteFrontmatter::new(KpId::Path(note.rel_path.clone()), title);
    fm.source = Some(source);
    fm.created = first_str(&map, CREATED_KEYS).map(normalize_timestamp);
    fm.updated = first_str(&map, UPDATED_KEYS).map(normalize_timestamp);
    fm.tags = read_tags(&map, TAG_KEYS);

    Some(Note {
        rel_path: note.rel_path.clone(),
        frontmatter: Frontmatter::Kp(fm),
        body: note.body.clone(),
    })
}

/// The first present, non-empty string among `keys`.
fn first_str(map: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = map.get(*key).and_then(Value::as_str) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }
    None
}

/// The first present string among `keys` that is an `http(s)` URL.
fn first_url(map: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = map.get(*key).and_then(Value::as_str) {
            let trimmed = s.trim();
            if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                return Some(trimmed.to_owned());
            }
        }
    }
    None
}

/// Tags as a `kp-note/v1` list: a YAML sequence of scalars, or a single
/// comma-separated string. Blank entries are dropped; order is preserved.
fn read_tags(map: &Value, keys: &[&str]) -> Vec<String> {
    for key in keys {
        match map.get(*key) {
            Some(Value::Sequence(items)) => {
                return items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::trim))
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
            Some(Value::String(csv)) => {
                return csv
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
            _ => {}
        }
    }
    Vec::new()
}

/// Coerce a clip's date into the RFC-3339-ish string the index sorts on.
/// A bare `YYYY-MM-DD` (what clippers often write) gains a midnight-UTC time
/// so it stays lexicographically ordered against full timestamps; anything
/// else passes through untouched (already a timestamp, or something we won't
/// second-guess).
fn normalize_timestamp(raw: String) -> String {
    let is_bare_date = raw.len() == 10
        && raw.as_bytes()[4] == b'-'
        && raw.as_bytes()[7] == b'-'
        && raw
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());
    if is_bare_date {
        format!("{raw}T00:00:00Z")
    } else {
        raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn foreign(rel_path: &str, yaml: &str, body: &str) -> Note {
        Note::parse(rel_path, &format!("---\n{yaml}---\n{body}")).expect("parses")
    }

    /// A typical read-and-save clipper's default template shape.
    const WEB_VIEWER: &str = "title: Attention Is All You Need\n\
source: https://arxiv.org/abs/1706.03762\n\
author: Vaswani et al.\n\
published: 2017-06-12\n\
created: 2026-07-06\n\
description: The transformer paper\n\
tags:\n  - clippings\n  - ml\n";

    #[test]
    fn adapts_a_web_viewer_note_onto_kp_note_v1() {
        let note = foreign("clips/attention.md", WEB_VIEWER, "The dominant models…\n");
        let adapted = adapt_webclip(&note).expect("recognized as a web clip");
        let Frontmatter::Kp(fm) = &adapted.frontmatter else {
            panic!("expected a synthesized kp-note view");
        };
        assert_eq!(fm.kp_id, KpId::Path("clips/attention.md".to_owned()));
        assert_eq!(fm.title, "Attention Is All You Need");
        assert_eq!(
            fm.source.as_deref(),
            Some("https://arxiv.org/abs/1706.03762")
        );
        assert_eq!(fm.tags, vec!["clippings".to_owned(), "ml".to_owned()]);
        // A bare date is lifted to midnight UTC so recency ordering holds.
        assert_eq!(fm.created.as_deref(), Some("2017-06-12T00:00:00Z"));
        assert_eq!(fm.updated, None);
        // The body rides along untouched.
        assert_eq!(adapted.body, "The dominant models…\n");
    }

    #[test]
    fn honours_url_and_comma_tag_aliases() {
        let note = foreign(
            "clips/x.md",
            "url: https://example.com/post\ntags: rust, systems, wasm\n",
            "body\n",
        );
        let fm = match adapt_webclip(&note).expect("web clip").frontmatter {
            Frontmatter::Kp(fm) => fm,
            _ => panic!("kp view"),
        };
        assert_eq!(fm.source.as_deref(), Some("https://example.com/post"));
        assert_eq!(
            fm.tags,
            vec!["rust".to_owned(), "systems".to_owned(), "wasm".to_owned()]
        );
        // No title key → the file stem.
        assert_eq!(fm.title, "x");
    }

    #[test]
    fn preserves_a_full_timestamp_and_reads_updated() {
        let note = foreign(
            "clips/y.md",
            "source: https://example.com\ncreated: 2026-01-02T03:04:05Z\nupdated: 2026-02-03\n",
            "b\n",
        );
        let fm = match adapt_webclip(&note).expect("web clip").frontmatter {
            Frontmatter::Kp(fm) => fm,
            _ => panic!("kp view"),
        };
        assert_eq!(fm.created.as_deref(), Some("2026-01-02T03:04:05Z"));
        assert_eq!(fm.updated.as_deref(), Some("2026-02-03T00:00:00Z"));
    }

    #[test]
    fn rejects_non_webclip_foreign_notes() {
        // No http(s) source → not a web clip (someone else's plain metadata).
        let plain = foreign("notes/idea.md", "status: draft\npriority: high\n", "x\n");
        assert!(adapt_webclip(&plain).is_none());
        // A non-URL source is not the signal we key on.
        let bookish = foreign("notes/quote.md", "source: A paperback\n", "x\n");
        assert!(adapt_webclip(&bookish).is_none());
    }

    #[test]
    fn never_claims_a_curio_note() {
        let curio = foreign(
            "curio/a.md",
            "schema: curio.frontmatter.v1\nsource: https://example.com\ntitle: T\n",
            "b\n",
        );
        assert!(adapt_webclip(&curio).is_none());
    }

    #[test]
    fn ignores_kp_and_bare_notes() {
        let kp = Note::parse(
            "n.md",
            "---\nkp_id: kp:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d\nkp_schema: kp-note/v1\ntitle: T\n---\nb\n",
        )
        .expect("parses");
        assert!(adapt_webclip(&kp).is_none());
        let bare = Note::parse("n.md", "just a body, no frontmatter\n").expect("parses");
        assert!(adapt_webclip(&bare).is_none());
    }
}
