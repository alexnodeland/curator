//! The Curio adapter — adapter, not template.
//!
//! Curio exports vanilla `curio.frontmatter.v1` notes; it is NOT required
//! to know kp-note exists. This adapter reads those notes natively and
//! maps `curio_id` → `kp_id: curio:<id>` in memory, validating every
//! document at the boundary against the **vendored, sha-pinned** schemas
//! under `contracts/vendor/curio/` (embedded at compile time — the
//! adapter's behavior is a function of a recorded upstream commit, never
//! of a live sibling checkout).
//!
//! Boundary rules (binding, from `contracts/kp-note/v1.md`):
//! - schema violations are **warnings + skip**, never crashes;
//! - `.curio/**` is Curio-owned; the plane never writes there;
//! - `.curio/manifest.json` is the write-ownership oracle
//!   ([`CurioManifest::owned_paths`] feeds the proposals validator);
//! - the managed region (`curio:managed` markers) is Curio's; the whole
//!   note text is indexed, but the managed/companion split is exposed so
//!   enrichment stays outside the region.

use std::collections::BTreeMap;

use kp_core::note::{Frontmatter, Note, NoteFrontmatter};
use kp_core::{Checksum, KpId, Vault};
use serde::Deserialize;

/// The `schema` frontmatter value of a Curio note.
pub const CURIO_FRONTMATTER_SCHEMA: &str = "curio.frontmatter.v1";
/// The `schema` value of `.curio/manifest.json`.
pub const CURIO_MANIFEST_SCHEMA: &str = "curio.manifest.v1";
/// The `schema` envelope value of one events-log line.
pub const CURIO_EVENTS_SCHEMA: &str = "curio.events.v1";

// Managed-region markers (v1) — the canonical definitions moved to
// kp-core (the proposals/v1 write primitive guards on them too);
// re-exported here so adapter callers keep one import path.
pub use kp_core::managed::{MANAGED_BEGIN, MANAGED_END};

/// Vendored schema bytes — the pinned boundary (see `contracts/vendor/curio/PIN`).
const FRONTMATTER_SCHEMA_JSON: &str =
    include_str!("../../../contracts/vendor/curio/frontmatter.v1.json");
const EVENTS_SCHEMA_JSON: &str = include_str!("../../../contracts/vendor/curio/events.v1.json");

/// The Curio adapter: compiled vendored-schema validators.
pub struct CurioAdapter {
    frontmatter: jsonschema::Validator,
    events: jsonschema::Validator,
}

impl std::fmt::Debug for CurioAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CurioAdapter")
            .field("frontmatter", &CURIO_FRONTMATTER_SCHEMA)
            .field("events", &CURIO_EVENTS_SCHEMA)
            .finish()
    }
}

impl Default for CurioAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl CurioAdapter {
    /// Compile the vendored schemas.
    ///
    /// # Panics
    /// Only if a vendored schema file is itself invalid — impossible for a
    /// correctly synced vendor dir (pinned, tested, compile-time embedded).
    #[must_use]
    pub fn new() -> Self {
        let compile = |raw: &str, name: &str| {
            let doc: serde_json::Value =
                serde_json::from_str(raw).unwrap_or_else(|e| panic!("vendored {name}: {e}"));
            jsonschema::validator_for(&doc)
                .unwrap_or_else(|e| panic!("vendored {name} does not compile: {e}"))
        };
        Self {
            frontmatter: compile(FRONTMATTER_SCHEMA_JSON, "frontmatter.v1.json"),
            events: compile(EVENTS_SCHEMA_JSON, "events.v1.json"),
        }
    }

    /// Adapt one parsed note. Never fails hard: non-Curio notes pass
    /// through as [`CurioAdapt::NotCurio`], schema violations come back as
    /// [`CurioAdapt::Invalid`] for the caller to warn on and skip.
    ///
    /// An explicit `kp_id` always wins: a note carrying kp-note
    /// frontmatter is NotCurio even if Curio keys ride along in `extra`.
    #[must_use]
    pub fn adapt(&self, note: &Note) -> CurioAdapt {
        let Frontmatter::Foreign(yaml) = &note.frontmatter else {
            return CurioAdapt::NotCurio;
        };
        let Ok(loose) = serde_yaml::from_str::<serde_yaml::Value>(yaml) else {
            return CurioAdapt::NotCurio; // unparseable YAML never reaches here (Note::parse rejects it)
        };
        if loose.get("schema").and_then(|v| v.as_str()) != Some(CURIO_FRONTMATTER_SCHEMA) {
            return CurioAdapt::NotCurio;
        }

        // The note claims to be Curio's: validate against the vendored pin.
        let as_json = match serde_json::to_value(&loose) {
            Ok(v) => v,
            Err(e) => {
                return CurioAdapt::Invalid {
                    path: note.rel_path.clone(),
                    warnings: vec![format!("frontmatter is not JSON-representable: {e}")],
                };
            }
        };
        let warnings: Vec<String> = self
            .frontmatter
            .iter_errors(&as_json)
            .map(|err| format!("{}: {err}", err.instance_path()))
            .collect();
        if !warnings.is_empty() {
            return CurioAdapt::Invalid {
                path: note.rel_path.clone(),
                warnings,
            };
        }
        let fm: CurioFrontmatter = match serde_yaml::from_str(yaml) {
            Ok(fm) => fm,
            Err(e) => {
                return CurioAdapt::Invalid {
                    path: note.rel_path.clone(),
                    warnings: vec![format!("schema-valid but undeserializable: {e}")],
                };
            }
        };

        // The in-memory kp-note view: curio_id → kp_id curio:<id>. The
        // declared checksum rides along as producer metadata (change
        // detection keys on the full note, not on it). NEVER written back.
        let mut kp_fm = NoteFrontmatter::new(KpId::Curio(fm.curio_id.clone()), fm.title.clone());
        kp_fm.checksum = Some(fm.checksum.clone());
        kp_fm.tags = fm.tags.clone();
        kp_fm.source = Some(fm.source.clone());
        kp_fm.created = Some(fm.saved.clone());
        let kp_note = Note {
            rel_path: note.rel_path.clone(),
            frontmatter: Frontmatter::Kp(kp_fm),
            body: note.body.clone(),
        };
        let split = split_managed(&note.body);
        CurioAdapt::Adapted(Box::new(AdaptedCurio {
            frontmatter: fm,
            kp_note,
            split,
        }))
    }

    /// Parse + validate one events-log line against the vendored
    /// `curio.events.v1` schema. `Err` carries the warning message —
    /// callers warn and skip, never crash.
    pub fn parse_event(&self, line: &str) -> Result<CurioEvent, String> {
        let value: serde_json::Value =
            serde_json::from_str(line).map_err(|e| format!("not JSON: {e}"))?;
        let errors: Vec<String> = self
            .events
            .iter_errors(&value)
            .map(|err| format!("{}: {err}", err.instance_path()))
            .collect();
        if !errors.is_empty() {
            return Err(format!("schema violation: {}", errors.join("; ")));
        }
        serde_json::from_value(value).map_err(|e| format!("undeserializable event: {e}"))
    }
}

/// The outcome of [`CurioAdapter::adapt`].
#[derive(Debug)]
pub enum CurioAdapt {
    /// Not a Curio note — ingest it by the ordinary rules.
    NotCurio,
    /// A valid Curio note, adapted to its kp-note view.
    Adapted(Box<AdaptedCurio>),
    /// Claims `curio.frontmatter.v1` but violates the vendored schema:
    /// warn + skip.
    Invalid { path: String, warnings: Vec<String> },
}

/// A Curio note after adaptation.
#[derive(Debug, Clone)]
pub struct AdaptedCurio {
    /// The validated Curio frontmatter, as declared.
    pub frontmatter: CurioFrontmatter,
    /// The in-memory kp-note view (identity `curio:<id>`, declared
    /// checksum as change token). Index this; never write it to disk.
    pub kp_note: Note,
    /// The managed/companion split, when the managed markers are present.
    pub split: Option<ManagedSplit>,
}

/// `curio.frontmatter.v1`, typed (validated against the vendored schema
/// BEFORE deserialization — this struct trusts the schema).
#[derive(Debug, Clone, Deserialize)]
pub struct CurioFrontmatter {
    pub schema: String,
    pub curio_id: String,
    pub title: String,
    pub source: String,
    #[serde(default)]
    pub feed: Option<String>,
    #[serde(default)]
    pub feed_title: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub published: Option<String>,
    pub saved: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub checksum: Checksum,
    #[serde(default)]
    pub lang: Option<String>,
    #[serde(default)]
    pub word_count: Option<u64>,
}

/// The managed/companion split of a Curio note body. Reconstruction
/// invariant: `before + BEGIN + managed + END + after == body`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedSplit {
    /// Companion content above the managed region (usually empty).
    pub before: String,
    /// The Curio-owned extracted article markdown (between the markers —
    /// exactly the bytes the declared checksum covers).
    pub managed: String,
    /// Companion content below the managed region — the KP enrichment
    /// zone. Curio re-exports preserve it byte-for-byte.
    pub after: String,
}

/// Split a body on the v1 managed-region markers. `None` when the markers
/// are absent or malformed (missing end, end before begin).
#[must_use]
pub fn split_managed(body: &str) -> Option<ManagedSplit> {
    let begin = body.find(MANAGED_BEGIN)?;
    let after_begin = begin + MANAGED_BEGIN.len();
    let end_rel = body[after_begin..].find(MANAGED_END)?;
    let end = after_begin + end_rel;
    Some(ManagedSplit {
        before: body[..begin].to_owned(),
        managed: body[after_begin..end].to_owned(),
        after: body[end + MANAGED_END.len()..].to_owned(),
    })
}

/// One `curio.events.v1` envelope (validated before deserialization).
#[derive(Debug, Clone, Deserialize)]
pub struct CurioEvent {
    pub schema: String,
    pub event_id: String,
    pub ts: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub payload: serde_json::Value,
}

impl CurioEvent {
    /// The `curio_id` carried by every `article.*` payload.
    #[must_use]
    pub fn curio_id(&self) -> Option<&str> {
        self.payload.get("curio_id").and_then(|v| v.as_str())
    }
}

/// `.curio/manifest.json` — the export-idempotency oracle, read by the KP
/// as the write-ownership record: manifest-listed paths are Curio-owned
/// at the managed-region level.
#[derive(Debug, Clone, Deserialize)]
pub struct CurioManifest {
    pub schema: String,
    pub notes: BTreeMap<String, ManifestEntry>,
}

/// One export record in the manifest.
#[derive(Debug, Clone, Deserialize)]
pub struct ManifestEntry {
    /// Note path relative to the destination (= vault) root.
    pub path: String,
    /// Managed-region checksum as last exported. Change token, never identity.
    pub checksum: Checksum,
    pub exported_at: String,
}

impl CurioManifest {
    /// Load `.curio/manifest.json` from a vault. `Ok(None)` when absent
    /// (Curio has never exported here); `Err` carries a warning message —
    /// a malformed manifest is warned about, never fatal.
    pub fn load(vault: &Vault) -> Result<Option<Self>, String> {
        let raw = match vault.read(".curio/manifest.json") {
            Ok(raw) => raw,
            Err(_) => return Ok(None),
        };
        let manifest: Self = serde_json::from_str(&raw)
            .map_err(|e| format!("malformed .curio/manifest.json: {e}"))?;
        if manifest.schema != CURIO_MANIFEST_SCHEMA {
            return Err(format!(
                "manifest schema {:?} is not {CURIO_MANIFEST_SCHEMA:?}",
                manifest.schema
            ));
        }
        Ok(Some(manifest))
    }

    /// The write-ownership oracle: every vault-relative path Curio owns
    /// (at the managed-region level). Feeds the proposals validator.
    #[must_use]
    pub fn owned_paths(&self) -> Vec<&str> {
        self.notes.values().map(|e| e.path.as_str()).collect()
    }

    /// Is this vault-relative path Curio-owned?
    #[must_use]
    pub fn owns(&self, rel_path: &str) -> bool {
        self.notes.values().any(|e| e.path == rel_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURIO_ID: &str = "0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d";
    const SHA: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

    fn curio_note_content() -> String {
        format!(
            "---\n\
             schema: curio.frontmatter.v1\n\
             curio_id: {CURIO_ID}\n\
             title: \"Async patterns\"\n\
             source: \"https://example.com/async\"\n\
             feed: \"https://example.com/feed.xml\"\n\
             feed_title: \"Example Blog\"\n\
             author: \"Jane Doe\"\n\
             published: 2026-07-01T12:00:00Z\n\
             saved: 2026-07-03T09:15:00.123Z\n\
             tags: [rust, async]\n\
             checksum: \"sha256:{SHA}\"\n\
             lang: \"en\"\n\
             word_count: 42\n\
             ---\n\
             {MANAGED_BEGIN}\n\
             # Async patterns\n\nextracted article text\n\
             {MANAGED_END}\n\
             \nMy own companion thoughts.\n"
        )
    }

    #[test]
    fn adapts_a_valid_curio_note() {
        let adapter = CurioAdapter::new();
        let note = Note::parse("curio/async.md", &curio_note_content()).expect("parses");
        let CurioAdapt::Adapted(adapted) = adapter.adapt(&note) else {
            panic!("expected adaptation");
        };
        assert_eq!(adapted.frontmatter.curio_id, CURIO_ID);
        assert_eq!(
            adapted.kp_note.kp_id().to_string(),
            format!("curio:{CURIO_ID}")
        );
        assert_eq!(adapted.kp_note.title(), "Async patterns");
        let Frontmatter::Kp(fm) = &adapted.kp_note.frontmatter else {
            panic!("kp view must carry KP frontmatter");
        };
        assert_eq!(fm.checksum.as_ref().map(Checksum::hex), Some(SHA));
        assert_eq!(fm.tags, vec!["rust", "async"]);
        assert_eq!(fm.source.as_deref(), Some("https://example.com/async"));
        assert_eq!(fm.created.as_deref(), Some("2026-07-03T09:15:00.123Z"));
        // Whole text is the indexable body; the split is exposed.
        assert!(adapted.kp_note.body.contains("companion thoughts"));
        let split = adapted.split.expect("markers present");
        assert!(split.managed.contains("extracted article text"));
        assert!(split.after.contains("companion thoughts"));
        assert!(!split.managed.contains("companion"));
    }

    #[test]
    fn schema_violations_are_warnings_not_crashes() {
        let adapter = CurioAdapter::new();
        // Missing required `source`, bad curio_id shape.
        let raw = "---\nschema: curio.frontmatter.v1\ncurio_id: nope\ntitle: T\nfeed: null\npublished: null\nsaved: 2026-07-03T09:15:00.123Z\ntags: []\nchecksum: \"sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08\"\n---\nbody\n";
        let note = Note::parse("curio/bad.md", raw).expect("parses as a note");
        let CurioAdapt::Invalid { path, warnings } = adapter.adapt(&note) else {
            panic!("expected Invalid");
        };
        assert_eq!(path, "curio/bad.md");
        assert!(!warnings.is_empty());
    }

    #[test]
    fn non_curio_notes_pass_through() {
        let adapter = CurioAdapter::new();
        for raw in [
            "plain body, no frontmatter\n",
            "---\nsome: other-tool\n---\nbody\n",
            // Explicit kp_id wins even when a curio schema key rides along.
            "---\nkp_id: \"kp:abc\"\nkp_schema: kp-note/v1\ntitle: T\nschema: curio.frontmatter.v1\n---\nbody\n",
        ] {
            let note = Note::parse("n.md", raw).expect("parses");
            assert!(
                matches!(adapter.adapt(&note), CurioAdapt::NotCurio),
                "{raw:?} must be NotCurio"
            );
        }
    }

    #[test]
    fn managed_split_reconstructs_the_body() {
        let note = Note::parse("curio/x.md", &curio_note_content()).expect("parses");
        let split = split_managed(&note.body).expect("markers");
        let rebuilt = format!(
            "{}{MANAGED_BEGIN}{}{MANAGED_END}{}",
            split.before, split.managed, split.after
        );
        assert_eq!(rebuilt, note.body);
    }

    #[test]
    fn missing_or_malformed_markers_mean_no_split() {
        assert_eq!(split_managed("no markers at all"), None);
        assert_eq!(split_managed(&format!("{MANAGED_BEGIN}\nopen only")), None);
        // End before begin: the forward scan finds no end AFTER begin.
        assert_eq!(
            split_managed(&format!("{MANAGED_END}\n{MANAGED_BEGIN}\n")),
            None
        );
    }

    #[test]
    fn parses_and_validates_events() {
        let adapter = CurioAdapter::new();
        let good = format!(
            "{{\"schema\":\"curio.events.v1\",\"event_id\":\"01ARZ3NDEKTSV4RRFFQ69G5FAV\",\"ts\":\"2026-07-03T09:15:00.123Z\",\"type\":\"article.opened\",\"payload\":{{\"curio_id\":\"{CURIO_ID}\",\"dwell_ms\":1200}}}}"
        );
        let event = adapter.parse_event(&good).expect("valid");
        assert_eq!(event.kind, "article.opened");
        assert_eq!(event.curio_id(), Some(CURIO_ID));

        // Not JSON.
        assert!(adapter.parse_event("{oops").is_err());
        // JSON but schema-invalid: bad event_id, unknown type.
        let bad = format!(
            "{{\"schema\":\"curio.events.v1\",\"event_id\":\"nope\",\"ts\":\"2026-07-03T09:15:00.123Z\",\"type\":\"article.opened\",\"payload\":{{\"curio_id\":\"{CURIO_ID}\"}}}}"
        );
        assert!(adapter.parse_event(&bad).is_err());
        let bad_type = format!(
            "{{\"schema\":\"curio.events.v1\",\"event_id\":\"01ARZ3NDEKTSV4RRFFQ69G5FAW\",\"ts\":\"2026-07-03T09:15:00.123Z\",\"type\":\"article.exploded\",\"payload\":{{\"curio_id\":\"{CURIO_ID}\"}}}}"
        );
        assert!(adapter.parse_event(&bad_type).is_err());
        // Payload shape enforced per type: starred requires tags.
        let bad_payload = format!(
            "{{\"schema\":\"curio.events.v1\",\"event_id\":\"01ARZ3NDEKTSV4RRFFQ69G5FAX\",\"ts\":\"2026-07-03T09:15:00.123Z\",\"type\":\"article.starred\",\"payload\":{{\"curio_id\":\"{CURIO_ID}\"}}}}"
        );
        assert!(adapter.parse_event(&bad_payload).is_err());
    }

    #[test]
    fn manifest_loads_and_exposes_ownership() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("vault");
        std::fs::create_dir_all(root.join(".curio")).expect("mkdir");
        std::fs::write(
            root.join(".curio/manifest.json"),
            format!(
                "{{\n  \"schema\": \"curio.manifest.v1\",\n  \"notes\": {{\n    \"{CURIO_ID}\": {{ \"path\": \"curio/async.md\", \"checksum\": \"sha256:{SHA}\", \"exported_at\": \"2026-07-03T09:15:00.123Z\" }}\n  }}\n}}\n"
            ),
        )
        .expect("write");
        let vault = Vault::open(&root).expect("open");
        let manifest = CurioManifest::load(&vault)
            .expect("loads")
            .expect("present");
        assert_eq!(manifest.owned_paths(), vec!["curio/async.md"]);
        assert!(manifest.owns("curio/async.md"));
        assert!(!manifest.owns("notes/mine.md"));
    }

    #[test]
    fn absent_manifest_is_none_and_malformed_is_a_warning() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("vault");
        std::fs::create_dir_all(&root).expect("mkdir");
        let vault = Vault::open(&root).expect("open");
        assert!(
            CurioManifest::load(&vault)
                .expect("absent is fine")
                .is_none()
        );

        std::fs::create_dir_all(root.join(".curio")).expect("mkdir");
        std::fs::write(root.join(".curio/manifest.json"), "{not json").expect("write");
        assert!(CurioManifest::load(&vault).is_err());

        std::fs::write(
            root.join(".curio/manifest.json"),
            "{\"schema\": \"curio.manifest.v9\", \"notes\": {}}",
        )
        .expect("write");
        assert!(CurioManifest::load(&vault).is_err());
    }
}
