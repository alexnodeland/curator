//! `ingest()` — the incremental pipeline: walk → parse → adapt → chunk →
//! batch-embed → upsert → link → events tail.
//!
//! Identity derivation (kp-note/v1): an explicit `kp_id` wins; a
//! `curio.frontmatter.v1` note maps `curio_id` → `curio:<id>` via the
//! adapter (Curio never needs to know kp-note exists); everything else is
//! `path:<relpath>`.
//!
//! Change detection is by the FULL-note change token (`Note::change_token`
//! — canonical frontmatter + whole body) against the indexed
//! `notes.checksum`. Producer-declared frontmatter checksums are metadata
//! only: they cover just the producer's managed region, so keying change
//! detection on them would make user edits outside that region (companion
//! notes, wikilinks, tags) invisible to re-indexing. An unchanged
//! (token, path) pair skips the expensive path entirely — no chunking, no
//! embedding, no writes. All changed chunks of a run go through ONE
//! `Embedder::embed` call (batch-first trait).

use std::collections::{BTreeMap, BTreeSet};

use kp_core::{Checksum, KpConfig, Note, Vault};
use kp_index::{Chunk, ChunkParams, Embedder, EpochSource, Index, build_epoch_from};

use crate::chunker::chunk_markdown;
use crate::curio::{CurioAdapt, CurioAdapter, CurioManifest};
use crate::error::IngestError;
use crate::events::{TailReport, tail_events};
use crate::walker::walk_vault;

/// What one ingest run did — the `--json` summary of `kp ingest`.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct IngestReport {
    /// Notes considered (walked and not `.kpignore`d).
    pub scanned: usize,
    /// Notes (re)indexed: new or changed.
    pub ingested: usize,
    /// Notes skipped because checksum + path were unchanged.
    pub unchanged: usize,
    /// Notes skipped with warnings: parse failures, Curio schema
    /// violations, duplicate identities.
    pub skipped: usize,
    /// Files dropped by `.kpignore`.
    pub ignored: usize,
    /// Valid Curio notes among the scanned.
    pub curio_notes: usize,
    /// Index rows removed because their files vanished from the vault.
    pub removed: usize,
    /// Outgoing links recorded from (re)indexed notes.
    pub links: usize,
    /// The events-tail pass (present when `[curio].enabled`).
    pub events: Option<TailReport>,
}

/// A walked note resolved to its identity + change token.
#[derive(Debug)]
struct Prepared {
    /// The indexable kp-note view (Curio notes: the adapter's synthesis).
    note: Note,
    checksum: Checksum,
}

/// The prepared corpus: what both `ingest` and `rebuild` index.
#[derive(Debug, Default)]
struct Corpus {
    prepared: Vec<Prepared>,
    scanned: usize,
    ignored: usize,
    /// Parse failures + Curio schema violations + duplicate identities.
    skipped: usize,
    curio_notes: usize,
    /// Paths that still exist on disk and stayed eligible — used by the
    /// vanish-pruning pass. Parse-skipped files stay eligible: a
    /// transiently broken file must not evict its indexed rows.
    eligible_paths: BTreeSet<String>,
}

/// Walk + adapt + resolve identities (explicit `kp_id` > Curio adapter >
/// `path:` fallback) + dedupe. All note-level trouble is warn + skip.
fn prepare_corpus(vault: &Vault, adapter: &CurioAdapter) -> Result<Corpus, IngestError> {
    let walk = walk_vault(vault)?;
    let mut corpus = Corpus {
        ignored: walk.ignored.len(),
        skipped: walk.skipped.len(),
        eligible_paths: walk.skipped.iter().map(|(path, _)| path.clone()).collect(),
        ..Default::default()
    };
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    for walked in walk.notes {
        corpus.scanned += 1;
        corpus.eligible_paths.insert(walked.rel_path.clone());
        let note = match adapter.adapt(&walked.note) {
            CurioAdapt::Adapted(adapted) => {
                corpus.curio_notes += 1;
                adapted.kp_note
            }
            CurioAdapt::Invalid { path, warnings } => {
                for warning in &warnings {
                    tracing::warn!(note = %path, %warning, "skipping schema-violating Curio note");
                }
                corpus.skipped += 1;
                continue;
            }
            CurioAdapt::NotCurio => walked.note,
        };
        // The change token covers the WHOLE (possibly adapted) note —
        // mirrors what upsert_note_prechunked stores in notes.checksum.
        // Producer-declared frontmatter checksums are metadata, not the
        // token: they cover only the managed region, and user enrichment
        // outside it must still re-index.
        let p = Prepared {
            checksum: note.change_token(),
            note,
        };
        // Two files claiming one identity is a producer bug; first
        // (path-sorted) file wins deterministically, the rest warn.
        if !seen_ids.insert(p.note.kp_id().to_string()) {
            tracing::warn!(note = %p.note.rel_path, kp_id = %p.note.kp_id(),
                "skipping note with duplicate kp_id");
            corpus.skipped += 1;
            continue;
        }
        corpus.prepared.push(p);
    }
    Ok(corpus)
}

/// Run one incremental ingest per the module docs. Creates the index
/// (epoch 1) when none exists; opening an index built by a different
/// embedder fails, demanding an epoch rebuild.
pub fn ingest(config: &KpConfig, embedder: &dyn Embedder) -> Result<IngestReport, IngestError> {
    let vault = Vault::open(config.vault_path())?;
    let index_path = config.index_path();
    let mut index = if index_path.exists() {
        Index::open(&index_path, embedder)?
    } else {
        if let Some(parent) = index_path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| IngestError::Io {
                path: parent.to_owned(),
                source,
            })?;
        }
        Index::create(&index_path, embedder, 1)?
    };

    let adapter = CurioAdapter::new();
    // Surface a malformed ownership oracle early — it is not fatal to
    // indexing, but the proposals validator downstream needs to know.
    match CurioManifest::load(&vault) {
        Ok(_) => {}
        Err(warning) => tracing::warn!(%warning, "ignoring unreadable .curio manifest"),
    }

    let corpus = prepare_corpus(&vault, &adapter)?;
    let mut report = IngestReport {
        scanned: corpus.scanned,
        ignored: corpus.ignored,
        skipped: corpus.skipped,
        curio_notes: corpus.curio_notes,
        ..Default::default()
    };
    let prepared = &corpus.prepared;

    // Link-resolution map over the WHOLE corpus (changed or not).
    let targets = LinkTargets::build(prepared);

    // Change detection: unchanged (checksum, path) skips the expensive path.
    let params = ChunkParams::from_config(&config.index);
    let mut changed: Vec<(&Prepared, Vec<Chunk>)> = Vec::new();
    for p in prepared {
        let state = index.note_state(&p.note.kp_id().to_string())?;
        let unchanged = state.as_ref().is_some_and(|s| {
            s.path == p.note.rel_path && s.checksum.as_deref() == Some(&p.checksum.to_string())
        });
        if unchanged {
            report.unchanged += 1;
        } else {
            let chunks = chunk_markdown(&p.note.body, params);
            changed.push((p, chunks));
        }
    }

    // ONE batch embed call for every chunk of the run.
    let texts: Vec<&str> = changed
        .iter()
        .flat_map(|(_, chunks)| chunks.iter().map(|c| c.text.as_str()))
        .collect();
    let mut vectors = if texts.is_empty() {
        Vec::new()
    } else {
        embedder.embed(&texts)?
    }
    .into_iter();

    for (p, chunks) in &changed {
        let note_vectors: Vec<Vec<f32>> = vectors.by_ref().take(chunks.len()).collect();
        index.upsert_note_prechunked(&p.note, embedder, chunks, &note_vectors)?;
        report.ingested += 1;
        report.links += refresh_links(&mut index, &p.note, &targets)?;
    }

    // Prune rows whose files vanished (or became .kpignore'd).
    for (kp_id, path) in index.note_ids_and_paths()? {
        if !corpus.eligible_paths.contains(&path) {
            index.remove_note(&kp_id)?;
            report.removed += 1;
        }
    }

    // The events-tail pass rides every ingest when Curio is enabled.
    if config.curio.enabled {
        report.events = Some(tail_events(
            &config.curio_events_dir(),
            &adapter,
            &mut index,
        )?);
    }

    index.close()?;
    Ok(report)
}

/// Re-derive one note's outgoing edges from its body. Returns the number
/// of links recorded.
fn refresh_links(
    index: &mut Index,
    note: &Note,
    targets: &LinkTargets,
) -> Result<usize, IngestError> {
    let from_id = note.kp_id().to_string();
    index.clear_links_from(&from_id)?;
    let mut added = 0;
    for link in extract_links(&note.body) {
        let to_id = targets.resolve(&link);
        if to_id == from_id {
            continue; // self-links are noise
        }
        index.add_link(&from_id, &to_id, link.kind.as_str())?;
        added += 1;
    }
    Ok(added)
}

/// What one `kp index rebuild` did — the `--json` summary.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RebuildReport {
    /// The epoch counter stamped into the new file.
    pub epoch: i64,
    pub notes_indexed: usize,
    pub notes_skipped: usize,
    /// Files dropped by `.kpignore`.
    pub ignored: usize,
    /// Outgoing links recorded across the corpus.
    pub links: usize,
    /// The events-tail pass (present when `[curio].enabled`). Consumer
    /// state (cursors, dedupe, behavior rollups) carries forward across
    /// the epoch swap, so this pass simply resumes — it folds only events
    /// appended since the last tail, exactly like an incremental ingest.
    pub events: Option<TailReport>,
}

/// Blue/green epoch rebuild over the SAME corpus `ingest` indexes: the
/// walker (`.kpignore` honored), the Curio adapter (identities
/// preserved), the heading-aware chunker. Consumer state (cursors, event
/// dedupe, behavior rollups, digest log) carries forward through the
/// swap; after it, links are re-derived and — when Curio is enabled —
/// the events tail resumes from its carried cursors.
pub fn rebuild(config: &KpConfig, embedder: &dyn Embedder) -> Result<RebuildReport, IngestError> {
    let vault = Vault::open(config.vault_path())?;
    let adapter = CurioAdapter::new();
    let corpus = prepare_corpus(&vault, &adapter)?;
    let targets = LinkTargets::build(&corpus.prepared);

    let source = EpochSource {
        notes: corpus.prepared.iter().map(|p| p.note.clone()).collect(),
        skipped: corpus.skipped,
    };
    let epoch = build_epoch_from(config, embedder, &chunk_markdown, source)?;

    // The swap dropped all derived non-note state; rebuild it.
    let mut index = Index::open(config.index_path(), embedder)?;
    let mut links = 0;
    for p in &corpus.prepared {
        links += refresh_links(&mut index, &p.note, &targets)?;
    }
    let events = if config.curio.enabled {
        Some(tail_events(
            &config.curio_events_dir(),
            &adapter,
            &mut index,
        )?)
    } else {
        None
    };
    index.close()?;

    Ok(RebuildReport {
        epoch: epoch.epoch,
        notes_indexed: epoch.notes_indexed,
        notes_skipped: epoch.notes_skipped,
        ignored: corpus.ignored,
        links,
        events,
    })
}

/// A raw link occurrence in a note body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLink {
    /// Link target as written (display text / anchors stripped).
    pub target: String,
    pub kind: LinkKind,
}

/// How the link was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// `[[target]]` / `[[target|display]]`.
    Wiki,
    /// `[text](relative/path.md)`.
    Markdown,
}

impl LinkKind {
    /// The `links.kind` column value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            LinkKind::Wiki => "wikilink",
            LinkKind::Markdown => "markdown",
        }
    }
}

/// Extract wiki (`[[..]]`) and markdown (`[..](..)`) note links from a
/// body. External targets (schemes, anchors, mailto) and non-`.md`
/// markdown targets are not note links and are dropped.
#[must_use]
pub fn extract_links(body: &str) -> Vec<RawLink> {
    let mut out = Vec::new();

    // Wikilinks: [[target]], [[target|display]], [[target#heading]].
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else { break };
        let inner = &after[..end];
        let target = inner
            .split('|')
            .next()
            .unwrap_or_default()
            .split('#')
            .next()
            .unwrap_or_default()
            .trim();
        if !target.is_empty() && !inner.contains("[[") {
            out.push(RawLink {
                target: target.to_owned(),
                kind: LinkKind::Wiki,
            });
        }
        rest = &after[end + 2..];
    }

    // Markdown links: every "](" whose target parses as a relative .md path.
    for (i, _) in body.match_indices("](") {
        let after = &body[i + 2..];
        let Some(close) = after.find(')') else {
            continue;
        };
        let mut target = after[..close].trim();
        target = target
            .strip_prefix('<')
            .and_then(|t| t.strip_suffix('>'))
            .unwrap_or(target);
        // Drop a `"title"` suffix.
        target = target.split_whitespace().next().unwrap_or_default();
        // Anchors within the target are not part of the path.
        target = target.split('#').next().unwrap_or_default();
        if target.is_empty()
            || target.contains("://")
            || target.starts_with("mailto:")
            || !target.ends_with(".md")
        {
            continue;
        }
        out.push(RawLink {
            target: target.to_owned(),
            kind: LinkKind::Markdown,
        });
    }
    out
}

/// Resolves link targets to note identities: by vault-relative path, then
/// by file stem (both case-insensitive); unresolved targets become
/// deterministic `path:` ids so backlinks light up if the note appears.
#[derive(Debug, Default)]
struct LinkTargets {
    by_path: BTreeMap<String, String>,
    by_stem: BTreeMap<String, String>,
}

impl LinkTargets {
    fn build(prepared: &[Prepared]) -> Self {
        let mut t = Self::default();
        for p in prepared {
            let kp_id = p.note.kp_id().to_string();
            let path = p.note.rel_path.to_lowercase();
            t.by_path
                .entry(path.clone())
                .or_insert_with(|| kp_id.clone());
            let stem = path
                .rsplit('/')
                .next()
                .unwrap_or(&path)
                .trim_end_matches(".md")
                .to_owned();
            // First (path-sorted) note wins an ambiguous stem, deterministically.
            t.by_stem.entry(stem).or_insert(kp_id);
        }
        t
    }

    fn resolve(&self, link: &RawLink) -> String {
        let raw = link.target.trim_start_matches("./");
        let lower = raw.to_lowercase();
        let with_md = if lower.ends_with(".md") {
            lower.clone()
        } else {
            format!("{lower}.md")
        };
        if let Some(id) = self
            .by_path
            .get(&with_md)
            .or_else(|| self.by_path.get(&lower))
        {
            return id.clone();
        }
        let stem = lower
            .rsplit('/')
            .next()
            .unwrap_or(&lower)
            .trim_end_matches(".md");
        if let Some(id) = self.by_stem.get(stem) {
            return id.clone();
        }
        // Unresolved: a deterministic path-namespace id.
        let path = if raw.ends_with(".md") {
            raw.to_owned()
        } else {
            format!("{raw}.md")
        };
        format!("path:{path}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_wiki_and_markdown_links() {
        let body = "\
See [[notes/rust-notes|Rust]] and [[databases]] and [[topics#async]].
Also [a guide](guides/chunking.md) and [ext](https://example.com/x.md)
and [anchor](#local) and [mail](mailto:a@b.c) and ![img](pic.png)
and [titled](sub/doc.md \"a title\").
";
        let links = extract_links(body);
        assert_eq!(
            links,
            vec![
                RawLink {
                    target: "notes/rust-notes".into(),
                    kind: LinkKind::Wiki
                },
                RawLink {
                    target: "databases".into(),
                    kind: LinkKind::Wiki
                },
                RawLink {
                    target: "topics".into(),
                    kind: LinkKind::Wiki
                },
                RawLink {
                    target: "guides/chunking.md".into(),
                    kind: LinkKind::Markdown
                },
                RawLink {
                    target: "sub/doc.md".into(),
                    kind: LinkKind::Markdown
                },
            ]
        );
    }

    #[test]
    fn no_links_in_plain_text() {
        assert!(extract_links("nothing [here] or (there)\n").is_empty());
        assert!(extract_links("").is_empty());
    }

    fn prepared(rel: &str, content: &str) -> Prepared {
        let note = Note::parse(rel, content).expect("parses");
        let checksum = note.body_checksum();
        Prepared { note, checksum }
    }

    #[test]
    fn link_targets_resolve_path_stem_and_fallback() {
        let notes = vec![
            prepared("notes/rust-notes.md", "body"),
            prepared(
                "curio/async.md",
                "---\nkp_id: \"kp:0197aaaa-0000-7000-8000-000000000001\"\nkp_schema: kp-note/v1\ntitle: T\n---\nb",
            ),
        ];
        let t = LinkTargets::build(&notes);
        // Full-path resolution, case-insensitive, .md optional.
        for target in [
            "notes/rust-notes.md",
            "notes/rust-notes",
            "Notes/Rust-Notes",
        ] {
            assert_eq!(
                t.resolve(&RawLink {
                    target: target.into(),
                    kind: LinkKind::Wiki
                }),
                "path:notes/rust-notes.md",
                "{target}"
            );
        }
        // Stem resolution reaches the minted identity.
        assert_eq!(
            t.resolve(&RawLink {
                target: "async".into(),
                kind: LinkKind::Wiki
            }),
            "kp:0197aaaa-0000-7000-8000-000000000001"
        );
        // Unresolved: deterministic path fallback.
        assert_eq!(
            t.resolve(&RawLink {
                target: "future note".into(),
                kind: LinkKind::Wiki
            }),
            "path:future note.md"
        );
    }
}
