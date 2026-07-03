//! The Zotero Web API v3 item model — the subset the mapper consumes.
//!
//! Every field defaults: the API's per-type `data` payloads carry wildly
//! different key sets, and unknown keys are simply ignored. This model is
//! INTERNAL — the published surface is the kp-note/v1 files the mapper
//! writes, never these structs.

use serde::Deserialize;

/// One item as returned by `GET /users/{id}/items` (`format=json`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Item {
    /// The 8-character item key — the `zotero:<itemKey>` identity.
    pub key: String,
    /// The item's version (monotone per library).
    pub version: i64,
    /// The editable payload.
    pub data: ItemData,
}

impl Item {
    /// The item key, preferring `data.key` (always present on full
    /// payloads) over the envelope `key`.
    #[must_use]
    pub fn key(&self) -> &str {
        if self.data.key.is_empty() {
            &self.key
        } else {
            &self.data.key
        }
    }

    /// Is this an attachment item?
    #[must_use]
    pub fn is_attachment(&self) -> bool {
        self.data.item_type == "attachment"
    }

    /// Zotero child notes and annotations — never mapped to vault notes
    /// (their content belongs to the parent's reading workflow, not the
    /// bibliography).
    #[must_use]
    pub fn is_note_or_annotation(&self) -> bool {
        matches!(self.data.item_type.as_str(), "note" | "annotation")
    }

    /// Top-level = no `parentItem`.
    #[must_use]
    pub fn is_top_level(&self) -> bool {
        self.data.parent_item.is_empty()
    }
}

/// The `data` payload of an item. Fields beyond this subset are ignored.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ItemData {
    pub key: String,
    pub version: i64,
    #[serde(rename = "itemType")]
    pub item_type: String,
    pub title: String,
    pub creators: Vec<Creator>,
    #[serde(rename = "abstractNote")]
    pub abstract_note: String,
    /// Free-form publication date ("2024-05-01", "May 2024", "2024"...).
    pub date: String,
    #[serde(rename = "dateAdded")]
    pub date_added: String,
    #[serde(rename = "dateModified")]
    pub date_modified: String,
    pub url: String,
    pub tags: Vec<Tag>,

    // journalArticle
    #[serde(rename = "publicationTitle")]
    pub publication_title: String,
    pub volume: String,
    pub issue: String,
    pub pages: String,
    #[serde(rename = "DOI")]
    pub doi: String,

    // book
    pub publisher: String,
    pub place: String,
    #[serde(rename = "ISBN")]
    pub isbn: String,

    // webpage
    #[serde(rename = "websiteTitle")]
    pub website_title: String,
    #[serde(rename = "accessDate")]
    pub access_date: String,

    // report
    pub institution: String,
    #[serde(rename = "reportNumber")]
    pub report_number: String,
    #[serde(rename = "reportType")]
    pub report_type: String,

    // attachment
    #[serde(rename = "parentItem")]
    pub parent_item: String,
    #[serde(rename = "linkMode")]
    pub link_mode: String,
    #[serde(rename = "contentType")]
    pub content_type: String,
    pub filename: String,
}

/// One creator entry.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Creator {
    #[serde(rename = "creatorType")]
    pub creator_type: String,
    #[serde(rename = "firstName")]
    pub first_name: String,
    #[serde(rename = "lastName")]
    pub last_name: String,
    /// Single-field creators (institutions, mononyms).
    pub name: String,
}

impl Creator {
    /// Human display name: the single-field `name` when present, else
    /// "First Last" with empty halves dropped.
    #[must_use]
    pub fn display_name(&self) -> String {
        let single = self.name.trim();
        if !single.is_empty() {
            return single.to_owned();
        }
        let mut parts: Vec<&str> = Vec::new();
        let first = self.first_name.trim();
        let last = self.last_name.trim();
        if !first.is_empty() {
            parts.push(first);
        }
        if !last.is_empty() {
            parts.push(last);
        }
        parts.join(" ")
    }
}

/// One tag entry (`{"tag": "rust", "type": 1}` — type ignored).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Tag {
    pub tag: String,
}

/// The `GET /users/{id}/deleted?since=` tombstone payload. Only item keys
/// matter to this producer; collections/searches/tags are ignored.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Deleted {
    pub items: Vec<String>,
}

/// The `GET /users/{id}/items/{key}/fulltext` payload.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Fulltext {
    pub content: String,
    #[serde(rename = "indexedPages")]
    pub indexed_pages: Option<u64>,
    #[serde(rename = "totalPages")]
    pub total_pages: Option<u64>,
    #[serde(rename = "indexedChars")]
    pub indexed_chars: Option<u64>,
    #[serde(rename = "totalChars")]
    pub total_chars: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creator_display_names() {
        let two_field = Creator {
            creator_type: "author".into(),
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
            name: String::new(),
        };
        assert_eq!(two_field.display_name(), "Ada Lovelace");

        let single = Creator {
            name: "Analytical Engine Society".into(),
            ..Default::default()
        };
        assert_eq!(single.display_name(), "Analytical Engine Society");

        let last_only = Creator {
            last_name: "Plato".into(),
            ..Default::default()
        };
        assert_eq!(last_only.display_name(), "Plato");
    }

    #[test]
    fn item_key_prefers_data_key() {
        let mut item = Item {
            key: "ENVKEY01".into(),
            ..Default::default()
        };
        assert_eq!(item.key(), "ENVKEY01");
        item.data.key = "DATAKEY1".into();
        assert_eq!(item.key(), "DATAKEY1");
    }

    #[test]
    fn classification_helpers() {
        let mut item = Item::default();
        item.data.item_type = "attachment".into();
        item.data.parent_item = "PARENT01".into();
        assert!(item.is_attachment());
        assert!(!item.is_top_level());

        item.data.item_type = "note".into();
        assert!(item.is_note_or_annotation());

        item.data.item_type = "journalArticle".into();
        item.data.parent_item = String::new();
        assert!(item.is_top_level());
        assert!(!item.is_attachment());
        assert!(!item.is_note_or_annotation());
    }

    #[test]
    fn unknown_data_keys_are_ignored() {
        let raw = r#"{
            "key": "ABCD2345",
            "version": 12,
            "library": {"type": "user", "id": 1},
            "meta": {"numChildren": 2},
            "data": {
                "key": "ABCD2345",
                "version": 12,
                "itemType": "journalArticle",
                "title": "T",
                "collections": ["X"],
                "relations": {},
                "extra": "stuff"
            }
        }"#;
        let item: Item = serde_json::from_str(raw).expect("parses");
        assert_eq!(item.key(), "ABCD2345");
        assert_eq!(item.data.item_type, "journalArticle");
    }
}
