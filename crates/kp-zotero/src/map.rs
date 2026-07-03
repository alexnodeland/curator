//! Item → kp-note/v1 mapping.
//!
//! Every mapped file is KP-produced (`kp_schema` present), keyed
//! `kp_id: zotero:<itemKey>`, and named `<itemKey>.md` inside the
//! configured notes dir — the key is the identity, so the path never
//! churns when a title changes. The whole rendered body sits inside the
//! `kp-zotero:managed` region; the frontmatter `checksum` is the change
//! token over exactly that region (mirroring Curio's declared-checksum
//! semantics, so ingest change-detection keys on machine content).
//!
//! Field mapping (task-fixed): `title`; creators → an author list in the
//! body; `date` → `created` (free-form publication dates normalized to
//! RFC 3339, falling back to `dateAdded`); `dateModified` → `updated`;
//! tags; `url` → `source`; `abstractNote` → an `## Abstract` section;
//! attachments → an `## Attachments` link list; fulltext (channel 2) → a
//! `## Fulltext` section truncated at the configured cap.

use kp_core::note::{Frontmatter, Note, NoteFrontmatter};
use kp_core::{Checksum, KpId};

use crate::item::{Creator, Item};
use crate::managed::compose_body;

/// Vault-relative path of an item's note: `<dir>/<KEY>.md`. The item key
/// (not the title) names the file so renames never orphan user edits.
#[must_use]
pub fn note_rel_path(notes_dir: &str, key: &str) -> String {
    let dir = notes_dir.trim_matches('/');
    if dir.is_empty() {
        format!("{key}.md")
    } else {
        format!("{dir}/{key}.md")
    }
}

/// One item mapped to its kp-note parts.
#[derive(Debug, Clone)]
pub struct MappedNote {
    /// The kp-note/v1 block (checksum already stamped over `managed`).
    pub frontmatter: NoteFrontmatter,
    /// The managed-region content (leading + trailing newline included).
    pub managed: String,
}

impl MappedNote {
    /// Render a fresh file: frontmatter + markers + empty user zones.
    #[must_use]
    pub fn fresh_content(&self) -> String {
        let note = Note {
            rel_path: String::new(),
            frontmatter: Frontmatter::Kp(self.frontmatter.clone()),
            body: compose_body("", &self.managed, "\n"),
        };
        note.to_markdown()
    }
}

/// Map one top-level item (+ its attachments + optional fulltext) to its
/// note parts.
#[must_use]
pub fn map_item(
    item: &Item,
    attachments: &[Item],
    fulltext: Option<&str>,
    fulltext_max_chars: usize,
) -> MappedNote {
    let managed = render_managed(item, attachments, fulltext, fulltext_max_chars);
    let data = &item.data;
    let title = non_empty(&data.title).unwrap_or_else(|| "(untitled)".to_owned());
    let mut fm = NoteFrontmatter::new(KpId::Zotero(item.key().to_owned()), title);
    fm.checksum = Some(Checksum::compute(managed.as_bytes()));
    fm.created = normalize_created(&data.date, &data.date_added);
    fm.updated = non_empty(&data.date_modified);
    fm.tags = data.tags.iter().filter_map(|t| non_empty(&t.tag)).collect();
    fm.source = non_empty(&data.url);
    MappedNote {
        frontmatter: fm,
        managed,
    }
}

/// Render the managed-region content for an item.
#[must_use]
pub fn render_managed(
    item: &Item,
    attachments: &[Item],
    fulltext: Option<&str>,
    fulltext_max_chars: usize,
) -> String {
    let data = &item.data;
    let mut blocks: Vec<String> = Vec::new();

    let title = non_empty(&data.title).unwrap_or_else(|| "(untitled)".to_owned());
    blocks.push(format!("# {title}"));

    if let Some(authors) = author_list(&data.creators) {
        blocks.push(format!("**Authors:** {authors}"));
    }

    blocks.push(type_line(item));

    if let Some(abstract_note) = non_empty(&data.abstract_note) {
        blocks.push(format!("## Abstract\n\n{abstract_note}"));
    }

    if !attachments.is_empty() {
        let lines: Vec<String> = attachments.iter().map(attachment_line).collect();
        blocks.push(format!("## Attachments\n\n{}", lines.join("\n")));
    }

    if let Some(text) = fulltext {
        let (body, truncated) = truncate_chars(text.trim_end(), fulltext_max_chars);
        let marker = if truncated {
            format!("\n\n*[fulltext truncated at {fulltext_max_chars} characters]*")
        } else {
            String::new()
        };
        blocks.push(format!("## Fulltext\n\n{body}{marker}"));
    }

    format!("\n{}\n", blocks.join("\n\n"))
}

/// The creators as one comma-joined author list; non-author roles are
/// annotated. `None` when there are no displayable creators.
#[must_use]
pub fn author_list(creators: &[Creator]) -> Option<String> {
    let names: Vec<String> = creators
        .iter()
        .filter_map(|c| {
            let name = c.display_name();
            if name.is_empty() {
                return None;
            }
            let role = c.creator_type.trim();
            if role.is_empty() || role == "author" {
                Some(name)
            } else {
                Some(format!("{name} ({})", humanize_camel(role).to_lowercase()))
            }
        })
        .collect();
    if names.is_empty() {
        None
    } else {
        Some(names.join(", "))
    }
}

/// The `**Type:** ...` line — per-type bibliographic detail with a generic
/// fallback for every other item type.
fn type_line(item: &Item) -> String {
    let data = &item.data;
    let mut line = String::from("**Type:** ");
    let mut parts: Vec<String> = Vec::new();
    match data.item_type.as_str() {
        "journalArticle" => {
            line.push_str("Journal article");
            if let Some(publication) = non_empty(&data.publication_title) {
                line.push_str(&format!(" — *{publication}*"));
            }
            if let Some(volume) = non_empty(&data.volume) {
                parts.push(format!("vol. {volume}"));
            }
            if let Some(issue) = non_empty(&data.issue) {
                parts.push(format!("no. {issue}"));
            }
            if let Some(pages) = non_empty(&data.pages) {
                parts.push(format!("pp. {pages}"));
            }
            if let Some(doi) = non_empty(&data.doi) {
                parts.push(format!("DOI [{doi}](https://doi.org/{doi})"));
            }
        }
        "book" => {
            line.push_str("Book");
            if let Some(publisher) = non_empty(&data.publisher) {
                line.push_str(&format!(" — {publisher}"));
            }
            if let Some(place) = non_empty(&data.place) {
                parts.push(place);
            }
            if let Some(isbn) = non_empty(&data.isbn) {
                parts.push(format!("ISBN {isbn}"));
            }
        }
        "webpage" => {
            line.push_str("Web page");
            if let Some(site) = non_empty(&data.website_title) {
                line.push_str(&format!(" — *{site}*"));
            }
            if let Some(accessed) = non_empty(&data.access_date) {
                parts.push(format!("accessed {accessed}"));
            }
        }
        "report" => {
            line.push_str("Report");
            if let Some(institution) = non_empty(&data.institution) {
                line.push_str(&format!(" — {institution}"));
            }
            if let Some(kind) = non_empty(&data.report_type) {
                parts.push(kind);
            }
            if let Some(number) = non_empty(&data.report_number) {
                parts.push(format!("no. {number}"));
            }
        }
        other => {
            // Generic fallback: the humanized item type, nothing invented.
            line.push_str(&humanize_camel(if other.is_empty() {
                "item"
            } else {
                other
            }));
        }
    }
    for part in parts {
        line.push_str(", ");
        line.push_str(&part);
    }
    line
}

/// One `## Attachments` bullet: a markdown link when the attachment has a
/// URL, else the stored-file name with its key.
fn attachment_line(att: &Item) -> String {
    let data = &att.data;
    let label = non_empty(&data.title)
        .or_else(|| non_empty(&data.filename))
        .unwrap_or_else(|| att.key().to_owned());
    match non_empty(&data.url) {
        Some(url) => format!("- [{label}]({url})"),
        None => format!("- {label} (stored: {})", att.key()),
    }
}

/// `date` → RFC 3339 `created`, falling back to `dateAdded` (already
/// RFC 3339 in the API). Partial publication dates are anchored to the
/// earliest instant they cover; unparseable free-form dates fall through.
#[must_use]
pub fn normalize_created(date: &str, date_added: &str) -> Option<String> {
    partial_to_rfc3339(date).or_else(|| non_empty(date_added))
}

/// Normalize `YYYY`, `YYYY-MM`, `YYYY-MM-DD` (also `/`-separated), or an
/// already-full timestamp. Anything else → `None`.
#[must_use]
pub fn partial_to_rfc3339(raw: &str) -> Option<String> {
    let t = raw.trim().replace('/', "-");
    // Already a full timestamp ("2026-07-03T09:15:00Z"-shaped)?
    let b = t.as_bytes();
    if t.len() >= 20
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && t[..4].chars().all(|c| c.is_ascii_digit())
    {
        return Some(t);
    }
    let segments: Vec<&str> = t.split('-').collect();
    let ok = |s: &str, len: usize| s.len() == len && s.chars().all(|c| c.is_ascii_digit());
    match segments.as_slice() {
        [y] if ok(y, 4) => Some(format!("{y}-01-01T00:00:00Z")),
        [y, m] if ok(y, 4) && ok(m, 2) => Some(format!("{y}-{m}-01T00:00:00Z")),
        [y, m, d] if ok(y, 4) && ok(m, 2) && ok(d, 2) => Some(format!("{y}-{m}-{d}T00:00:00Z")),
        _ => None,
    }
}

/// Truncate at a char boundary. Returns `(text, was_truncated)`.
#[must_use]
pub fn truncate_chars(s: &str, max_chars: usize) -> (String, bool) {
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => (s[..byte_idx].trim_end().to_owned(), true),
        None => (s.to_owned(), false),
    }
}

/// `camelCaseWords` → `Camel case words`.
#[must_use]
pub fn humanize_camel(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if i == 0 {
            out.extend(c.to_uppercase());
        } else if c.is_uppercase() {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_path_shapes() {
        assert_eq!(note_rel_path("zotero", "AB2C3DEF"), "zotero/AB2C3DEF.md");
        assert_eq!(note_rel_path("refs/lit/", "K"), "refs/lit/K.md");
        assert_eq!(note_rel_path("", "K"), "K.md");
    }

    #[test]
    fn date_normalization() {
        assert_eq!(
            partial_to_rfc3339("2024-05-01").as_deref(),
            Some("2024-05-01T00:00:00Z")
        );
        assert_eq!(
            partial_to_rfc3339("2024-05").as_deref(),
            Some("2024-05-01T00:00:00Z")
        );
        assert_eq!(
            partial_to_rfc3339("2024").as_deref(),
            Some("2024-01-01T00:00:00Z")
        );
        assert_eq!(
            partial_to_rfc3339("2024/05/01").as_deref(),
            Some("2024-05-01T00:00:00Z")
        );
        assert_eq!(
            partial_to_rfc3339("2026-07-03T09:15:00Z").as_deref(),
            Some("2026-07-03T09:15:00Z")
        );
        assert_eq!(partial_to_rfc3339("May 2024"), None);
        assert_eq!(partial_to_rfc3339(""), None);
        // Fallback order: date wins, then dateAdded, then nothing.
        assert_eq!(
            normalize_created("circa 1850", "2026-01-02T03:04:05Z").as_deref(),
            Some("2026-01-02T03:04:05Z")
        );
        assert_eq!(normalize_created("", ""), None);
    }

    #[test]
    fn truncation_is_char_safe() {
        let (t, cut) = truncate_chars("héllo wörld", 5);
        assert_eq!(t, "héllo");
        assert!(cut);
        let (t, cut) = truncate_chars("short", 100);
        assert_eq!(t, "short");
        assert!(!cut);
    }

    #[test]
    fn humanizes_item_types() {
        assert_eq!(humanize_camel("journalArticle"), "Journal article");
        assert_eq!(humanize_camel("conferencePaper"), "Conference paper");
        assert_eq!(humanize_camel("podcast"), "Podcast");
    }

    #[test]
    fn author_roles_are_annotated() {
        let creators = vec![
            Creator {
                creator_type: "author".into(),
                first_name: "Ada".into(),
                last_name: "Lovelace".into(),
                name: String::new(),
            },
            Creator {
                creator_type: "seriesEditor".into(),
                first_name: "Grace".into(),
                last_name: "Hopper".into(),
                name: String::new(),
            },
        ];
        assert_eq!(
            author_list(&creators).as_deref(),
            Some("Ada Lovelace, Grace Hopper (series editor)")
        );
        assert_eq!(author_list(&[]), None);
    }
}
