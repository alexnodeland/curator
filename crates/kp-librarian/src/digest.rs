//! The deterministic digest engine — ZERO LLM.
//!
//! Candidates = notes ingested or behaviorally active since the last
//! digest (`digest_log`). Score = `cosine(note chunk-mean embedding,
//! now.md anchor embedding) × exp(−age_days / half_life) × behavior
//! boost` (starred ×1.5, opened ×1.2 — events-derived signals). The top-k
//! render grouped by tag/source cluster, each with a wikilink, a one-line
//! extractive summary and a why-surfaced note; the lowest-scoring
//! leftovers render as a "quiet items" tail. Missing `now.md` = warn +
//! recency-only scoring.
//!
//! The digest is itself a kp-note (`kp_id: kp:<uuidv7>`, `tags:
//! [digest]`) written VIA a `proposals/v1` proposal — auto-applied only
//! when `--auto` is passed and the auto-apply gate admits it. Digests are
//! create-only and idempotent by date, and **byte-identical for identical
//! inputs**: the clock is injected, and the note's UUIDv7 derives its
//! non-timestamp bits from the digest content (see [`crate::uuid7`]).

use std::collections::BTreeMap;
use std::path::PathBuf;

use kp_core::note::{Frontmatter, Note, NoteFrontmatter};
use kp_core::time::{parse_rfc3339_utc, rfc3339_utc};
use kp_core::{KpConfig, KpId, ProposalFile, ProposalWriteError, Vault, VaultError};
use kp_index::embed::{EmbedError, cosine};
use kp_index::{ChunkParams, Embedder, Index, IndexError};
use kp_ingest::chunk_markdown;

use crate::proposals::{ApplyError, apply_proposal, auto_applicable};
use crate::uuid7::mint_uuid7;

/// The `author` stamped on librarian proposals.
pub const DIGEST_AUTHOR: &str = "kp-librarian";
/// Maximum quiet-tail entries.
const QUIET_MAX: usize = 5;
/// Extractive-summary clip length, characters.
const SUMMARY_MAX: usize = 160;

/// Errors from a digest run.
#[derive(Debug, thiserror::Error)]
pub enum DigestError {
    #[error(transparent)]
    Vault(#[from] VaultError),
    #[error(transparent)]
    Index(#[from] IndexError),
    #[error(transparent)]
    Embed(#[from] EmbedError),
    #[error(transparent)]
    Propose(#[from] ProposalWriteError),
    #[error(transparent)]
    Apply(#[from] ApplyError),
    /// No index yet — the digest ranks indexed notes.
    #[error("no index at {0} — run `kp ingest` first")]
    IndexMissing(PathBuf),
}

/// What one digest run did — the `--json` summary of `kp digest run`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DigestReport {
    /// The digest date (`YYYY-MM-DD`, from the injected clock).
    pub date: String,
    /// Vault-relative path of the digest note.
    pub note_path: String,
    /// The created proposal, when one was created.
    pub proposal_id: Option<String>,
    /// The minted digest identity, when a digest was rendered.
    pub kp_id: Option<String>,
    pub candidates: usize,
    /// Items surfaced (≤ top_k).
    pub items: usize,
    /// Quiet-tail items.
    pub quiet: usize,
    /// Whether the proposal was auto-applied.
    pub applied: bool,
    /// Set when the run was a no-op (digest already exists, no candidates).
    pub skipped: Option<String>,
    pub warnings: Vec<String>,
}

/// One scoring candidate, fully resolved (index reads done).
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub kp_id: String,
    pub path: String,
    pub title: String,
    pub tags: Vec<String>,
    pub source: Option<String>,
    /// Best available note timestamp (unix secs): frontmatter `updated`,
    /// else `created`, else the index's `ingested_at`.
    pub note_time: u64,
    /// The markdown body (for the extractive summary).
    pub body: String,
    /// Chunk-mean embedding, when the note has stored vectors.
    pub centroid: Option<Vec<f32>>,
    pub starred: bool,
    pub opened: bool,
    pub read_later: bool,
}

/// The injected knobs of one render.
#[derive(Debug, Clone, PartialEq)]
pub struct DigestParams {
    /// `YYYY-MM-DD`.
    pub date: String,
    /// The injected clock (unix secs) — age is measured from here.
    pub now_unix: u64,
    pub half_life_days: u32,
    pub top_k: usize,
    /// The now.md interest-anchor embedding; `None` = recency-only.
    pub anchor: Option<Vec<f32>>,
}

/// One scored candidate.
#[derive(Debug)]
struct Scored<'a> {
    candidate: &'a Candidate,
    /// `None` when there is no anchor or the note has no centroid.
    similarity: Option<f64>,
    age_days: f64,
    score: f64,
}

fn score_candidates<'a>(params: &DigestParams, candidates: &'a [Candidate]) -> Vec<Scored<'a>> {
    let half_life = f64::from(params.half_life_days.max(1));
    let mut scored: Vec<Scored<'a>> = candidates
        .iter()
        .map(|candidate| {
            let similarity = match (&params.anchor, &candidate.centroid) {
                (Some(anchor), Some(centroid)) => Some(f64::from(cosine(anchor, centroid))),
                _ => None,
            };
            let age_days = (params.now_unix.saturating_sub(candidate.note_time)) as f64 / 86_400.0;
            let recency = (-age_days / half_life).exp();
            let mut boost = 1.0;
            if candidate.starred {
                boost *= 1.5;
            }
            if candidate.opened {
                boost *= 1.2;
            }
            // No anchor (or no vectors) = recency-only: similarity term 1.
            let score = similarity.unwrap_or(1.0) * recency * boost;
            Scored {
                candidate,
                similarity,
                age_days,
                score,
            }
        })
        .collect();
    scored.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.candidate.kp_id.cmp(&b.candidate.kp_id))
    });
    scored
}

/// Render the digest markdown body. Pure and deterministic: identical
/// inputs produce identical bytes (the golden test pins this).
#[must_use]
pub fn render_digest_body(params: &DigestParams, candidates: &[Candidate]) -> String {
    let scored = score_candidates(params, candidates);
    let (top, rest) = scored.split_at(params.top_k.min(scored.len()));

    let mut out = format!("# Daily digest {}\n", params.date);
    if params.anchor.is_none() {
        out.push_str("\n*(no now.md anchor — recency-only ranking)*\n");
    }

    // Group the surfaced items by tag/source cluster, keeping score order
    // within a cluster; clusters render in name order (deterministic).
    let mut clusters: BTreeMap<String, Vec<&Scored<'_>>> = BTreeMap::new();
    for item in top {
        clusters
            .entry(cluster_key(item.candidate))
            .or_default()
            .push(item);
    }
    for (cluster, items) in &clusters {
        out.push_str(&format!("\n## {cluster}\n\n"));
        for item in items {
            let summary = extractive_summary(&item.candidate.body, SUMMARY_MAX);
            let summary = if summary.is_empty() {
                "(no preview)".to_owned()
            } else {
                summary
            };
            out.push_str(&format!(
                "- {} — {summary}\n  - why: {}\n",
                wikilink(item.candidate),
                why_surfaced(item)
            ));
        }
    }

    // The quiet tail: the lowest-scoring leftovers, fading from
    // now-similarity, quietest first.
    let mut quiet: Vec<&Scored<'_>> = rest.iter().collect();
    quiet.sort_by(|a, b| {
        a.score
            .total_cmp(&b.score)
            .then_with(|| a.candidate.kp_id.cmp(&b.candidate.kp_id))
    });
    quiet.truncate(QUIET_MAX);
    if !quiet.is_empty() {
        out.push_str("\n## Quiet items\n\nFading out of now-similarity:\n\n");
        for item in &quiet {
            let metric = match item.similarity {
                Some(sim) => format!("similarity {sim:.2}"),
                None => format!("{}d old", item.age_days as u64),
            };
            out.push_str(&format!("- {} — {metric}\n", wikilink(item.candidate)));
        }
    }
    out
}

/// How many items land in the surfaced (top-k) and quiet sections.
#[must_use]
pub fn digest_counts(params: &DigestParams, candidates: &[Candidate]) -> (usize, usize) {
    let surfaced = params.top_k.min(candidates.len());
    let quiet = (candidates.len() - surfaced).min(QUIET_MAX);
    (surfaced, quiet)
}

fn wikilink(candidate: &Candidate) -> String {
    let target = candidate
        .path
        .strip_suffix(".md")
        .unwrap_or(&candidate.path);
    format!("[[{target}|{}]]", candidate.title)
}

fn why_surfaced(item: &Scored<'_>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(sim) = item.similarity {
        parts.push(format!("similarity {sim:.2}"));
    }
    parts.push(format!("{}d old", item.age_days as u64));
    let mut flags: Vec<&str> = Vec::new();
    if item.candidate.starred {
        flags.push("starred");
    }
    if item.candidate.opened {
        flags.push("opened");
    }
    if item.candidate.read_later {
        flags.push("read later");
    }
    if !flags.is_empty() {
        parts.push(flags.join(", "));
    }
    parts.join(" · ")
}

/// The tag/source cluster a note belongs to: first tag, else the source
/// host, else `notes`.
fn cluster_key(candidate: &Candidate) -> String {
    if let Some(tag) = candidate.tags.first() {
        return tag.clone();
    }
    if let Some(source) = &candidate.source
        && let Some(host) = host_of(source)
    {
        return host;
    }
    "notes".to_owned()
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let host = rest.split('/').next().unwrap_or(rest);
    (!host.is_empty()).then(|| host.to_owned())
}

/// One-line extractive summary: the first paragraph that is not a
/// heading, HTML comment, fence or rule, whitespace-joined and clipped.
#[must_use]
pub fn extractive_summary(body: &str, max_chars: usize) -> String {
    let mut paragraph: Vec<&str> = Vec::new();
    let mut in_fence = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if trimmed.is_empty() {
            if !paragraph.is_empty() {
                break; // first real paragraph complete
            }
            continue;
        }
        if in_fence
            || trimmed.starts_with('#')
            || trimmed.starts_with("<!--")
            || trimmed.chars().all(|c| c == '-' || c == '=')
        {
            continue;
        }
        paragraph.push(trimmed);
    }
    clip(&paragraph.join(" "), max_chars)
}

fn clip(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        out.push('…');
    }
    out
}

/// Run one digest against the configured vault + index.
///
/// - `now_unix` is the injected clock — same inputs, same clock, same
///   bytes;
/// - `auto` applies the proposal immediately WHEN the auto-apply gate
///   admits it (pure additions under `digest_dir`, `kp:<uuidv7>`
///   identities); otherwise the proposal waits for `kp apply`.
///
/// Idempotent by date: an existing digest note (or `digest_log` row) for
/// the date makes the run a no-op.
pub fn run_digest(
    config: &KpConfig,
    embedder: &dyn Embedder,
    now_unix: u64,
    auto: bool,
) -> Result<DigestReport, DigestError> {
    let vault = Vault::open(config.vault_path())?;
    let index_path = config.index_path();
    if !index_path.exists() {
        return Err(DigestError::IndexMissing(index_path));
    }
    let mut index = Index::open(&index_path, embedder)?;
    let reader = index.reader()?;

    let now_str = rfc3339_utc(now_unix);
    let date = now_str[..10].to_owned();
    let digest_dir = config.librarian.digest_dir.trim_end_matches('/');
    let note_path = format!("{digest_dir}/{date}.md");
    let mut report = DigestReport {
        date: date.clone(),
        note_path: note_path.clone(),
        proposal_id: None,
        kp_id: None,
        candidates: 0,
        items: 0,
        quiet: 0,
        applied: false,
        skipped: None,
        warnings: Vec::new(),
    };

    // Create-only, idempotent by date.
    let last = reader.last_digest_entry()?;
    if vault.resolve(&note_path)?.exists()
        || last.as_ref().is_some_and(|entry| entry.digest_date == date)
    {
        report.skipped = Some(format!("digest for {date} already exists ({note_path})"));
        return Ok(report);
    }

    // Candidates: ingested or active since the last digest run.
    let cutoff = last.as_ref().map(|entry| entry.created.clone());
    let digest_prefix = format!("{digest_dir}/");
    let mut candidates: Vec<Candidate> = Vec::new();
    for summary in reader.active_since(cutoff.as_deref())? {
        // Never digest digests, and never surface the anchor itself.
        if summary.path.starts_with(&digest_prefix) || summary.path == config.librarian.now_path {
            continue;
        }
        let Some(record) = reader.get_note(&summary.kp_id)? else {
            continue;
        };
        let behavior = reader.behavior(&summary.kp_id)?;
        let note_time = [&record.updated, &record.created]
            .into_iter()
            .flatten()
            .find_map(|ts| parse_rfc3339_utc(ts))
            .or_else(|| parse_rfc3339_utc(&record.ingested_at))
            .unwrap_or(now_unix);
        candidates.push(Candidate {
            centroid: reader.note_centroid(&summary.kp_id)?,
            kp_id: record.kp_id,
            path: record.path,
            title: record.title,
            tags: record.tags,
            source: record.source,
            note_time,
            body: record.body,
            starred: behavior.as_ref().is_some_and(|b| b.starred),
            opened: behavior.as_ref().is_some_and(|b| b.opened_count > 0),
            read_later: behavior.as_ref().is_some_and(|b| b.read_later),
        });
    }
    report.candidates = candidates.len();
    if candidates.is_empty() {
        report.skipped = Some(match &cutoff {
            Some(since) => format!("no notes ingested or active since {since}"),
            None => "no notes in the index".to_owned(),
        });
        return Ok(report);
    }

    // The now.md interest anchor; missing = warn + recency-only.
    let anchor = match vault.read(&config.librarian.now_path) {
        Ok(raw) => {
            let body = Note::parse(config.librarian.now_path.as_str(), &raw)
                .map(|note| note.body)
                .unwrap_or(raw);
            let vector = embedder.embed_one(&body)?;
            if vector.iter().all(|x| *x == 0.0) {
                report.warnings.push(format!(
                    "{} is empty — recency-only scoring",
                    config.librarian.now_path
                ));
                None
            } else {
                Some(vector)
            }
        }
        Err(_) => {
            let warning = format!(
                "{} not found — recency-only scoring (set [librarian].now_path)",
                config.librarian.now_path
            );
            tracing::warn!("{warning}");
            report.warnings.push(warning);
            None
        }
    };

    let params = DigestParams {
        date: date.clone(),
        now_unix,
        half_life_days: config.librarian.half_life_days,
        top_k: config.librarian.top_k as usize,
        anchor,
    };
    let body = render_digest_body(&params, &candidates);
    (report.items, report.quiet) = digest_counts(&params, &candidates);

    // The digest is itself a kp-note, minted deterministically.
    let title = format!("Daily digest {date}");
    let uuid = mint_uuid7(now_unix * 1_000, body.as_bytes());
    let kp_id = KpId::Kp(uuid);
    let mut frontmatter = NoteFrontmatter::new(kp_id.clone(), title.clone());
    frontmatter.created = Some(now_str.clone());
    frontmatter.tags = vec!["digest".to_owned()];
    let content = Note {
        rel_path: note_path.clone(),
        frontmatter: Frontmatter::Kp(frontmatter),
        body,
    }
    .to_markdown();
    report.kp_id = Some(kp_id.to_string());

    // …written VIA a proposal — the only write path.
    let rationale = match &cutoff {
        Some(since) => format!(
            "{} note(s) ingested or active since {since}, scored against the now.md anchor.",
            candidates.len()
        ),
        None => format!(
            "First digest: {} note(s) scored against the now.md anchor.",
            candidates.len()
        ),
    };
    let proposals_dir = &config.vault.proposals_dir;
    let proposal = kp_core::create_proposal(
        &vault,
        proposals_dir,
        DIGEST_AUTHOR,
        &title,
        &rationale,
        &[ProposalFile {
            path: note_path.clone(),
            content: content.clone(),
        }],
    )?;
    report.proposal_id = Some(proposal.id.clone());

    if auto {
        // The gate re-derives everything from the stored patch — the same
        // check an untrusted proposal would face.
        let (_, patch) = kp_core::load_proposal(&vault, proposals_dir, &proposal.id)
            .map_err(|e| DigestError::Apply(ApplyError::Store(e)))?;
        let file_patches = crate::patch::parse_patch(&patch).map_err(|e| {
            DigestError::Apply(ApplyError::Rejected {
                id: proposal.id.clone(),
                reason: e.to_string(),
            })
        })?;
        let staged = vec![(note_path.clone(), content.clone())];
        if auto_applicable(&file_patches, digest_dir, &staged) {
            apply_proposal(&vault, proposals_dir, &proposal.id)?;
            report.applied = true;
            // Fold the fresh digest into the index so kp_digest_latest
            // serves it without waiting for the next ingest.
            let note = Note::parse(note_path.as_str(), &content)
                .expect("the librarian's own digest note always parses");
            let chunks = chunk_markdown(&note.body, ChunkParams::from_config(&config.index));
            let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
            let vectors = if texts.is_empty() {
                Vec::new()
            } else {
                embedder.embed(&texts)?
            };
            index.upsert_note_prechunked(&note, embedder, &chunks, &vectors)?;
            index.record_digest(&date, &kp_id.to_string(), &now_str)?;
        } else {
            report.warnings.push(format!(
                "proposal {} did not pass the auto-apply gate — left open for review",
                proposal.id
            ));
        }
    }
    index.close()?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kp_index::HashEmbedder;

    fn candidate(kp_id: &str, path: &str, title: &str) -> Candidate {
        Candidate {
            kp_id: kp_id.to_owned(),
            path: path.to_owned(),
            title: title.to_owned(),
            tags: Vec::new(),
            source: None,
            note_time: NOW,
            body: String::new(),
            centroid: None,
            starred: false,
            opened: false,
            read_later: false,
        }
    }

    /// 2026-07-03T09:15:00Z.
    const NOW: u64 = 1_783_070_100;

    fn embed(e: &HashEmbedder, text: &str) -> Option<Vec<f32>> {
        Some(e.embed_one(text).expect("embeds"))
    }

    /// A fixed fixture: two rust notes near the anchor (one starred, one
    /// aging), one off-topic cooking note.
    fn fixture(e: &HashEmbedder) -> (DigestParams, Vec<Candidate>) {
        let anchor = embed(e, "rust database sqlite embedded storage engine");
        let mut db = candidate("kp:a-db", "rust/db.md", "Rust databases");
        db.tags = vec!["rust".to_owned(), "databases".to_owned()];
        db.body = "# Rust databases\n\nEmbedded sqlite storage engine notes.\n".to_owned();
        db.centroid = embed(e, "rust database embedded sqlite storage engine queries");
        db.starred = true;
        db.opened = true;

        let mut older = candidate("kp:b-async", "rust/async.md", "Async rust");
        older.tags = vec!["rust".to_owned()];
        older.body = "Async rust database drivers for sqlite storage.\n".to_owned();
        older.centroid = embed(e, "rust async database sqlite storage drivers");
        older.note_time = NOW - 28 * 86_400; // two half-lives old

        let mut bread = candidate("kp:c-bread", "cooking/bread.md", "Bread");
        bread.source = Some("https://example.com/bread".to_owned());
        bread.body = "Sourdough hydration and proofing schedule.\n".to_owned();
        bread.centroid = embed(e, "sourdough flour hydration proofing oven");

        let params = DigestParams {
            date: "2026-07-03".to_owned(),
            now_unix: NOW,
            half_life_days: 14,
            top_k: 2,
            anchor,
        };
        (params, vec![db, older, bread])
    }

    #[test]
    fn golden_digest_is_byte_identical_and_structured() {
        let e = HashEmbedder::default();
        let (params, candidates) = fixture(&e);
        let first = render_digest_body(&params, &candidates);
        let second = render_digest_body(&params, &candidates);
        assert_eq!(first, second, "same inputs must render identical bytes");

        // Structure: title, cluster sections, ranked items, quiet tail.
        assert!(first.starts_with("# Daily digest 2026-07-03\n"), "{first}");
        assert!(first.contains("\n## rust\n"), "{first}");
        assert!(
            first.contains("- [[rust/db|Rust databases]] — Embedded sqlite storage engine notes."),
            "{first}"
        );
        assert!(first.contains("starred, opened"), "{first}");
        assert!(first.contains("\n## Quiet items\n"), "{first}");
        assert!(first.contains("[[cooking/bread|Bread]]"), "{first}");
        // The starred on-topic note outranks the aging one.
        let db_pos = first.find("rust/db").expect("db present");
        let async_pos = first.find("rust/async").expect("async present");
        assert!(db_pos < async_pos, "{first}");
        // Similarity + age render with fixed precision.
        assert!(first.contains("similarity 0."), "{first}");
        assert!(first.contains("28d old"), "{first}");
    }

    #[test]
    fn no_anchor_means_recency_only() {
        let e = HashEmbedder::default();
        let (mut params, candidates) = fixture(&e);
        params.anchor = None;
        let body = render_digest_body(&params, &candidates);
        assert!(body.contains("recency-only ranking"), "{body}");
        assert!(!body.contains("similarity 0"), "{body}");
        // Fresh notes beat old ones regardless of topic.
        let bread_pos = body.find("cooking/bread").expect("bread");
        let async_pos = body.find("rust/async").expect("async");
        assert!(bread_pos < async_pos, "{body}");
    }

    #[test]
    fn behavior_boost_lifts_a_note_over_a_peer() {
        let e = HashEmbedder::default();
        let anchor = embed(&e, "rust database sqlite embedded storage engine");
        let text = "rust database embedded sqlite storage engine";
        let mut plain = candidate("kp:plain", "a.md", "Plain");
        plain.centroid = embed(&e, text);
        let mut starred = candidate("kp:starred", "b.md", "Starred");
        starred.centroid = embed(&e, text);
        starred.starred = true;

        let params = DigestParams {
            date: "2026-07-03".to_owned(),
            now_unix: NOW,
            half_life_days: 14,
            top_k: 1,
            anchor,
        };
        // Identical similarity + age: the starred one must win the only slot.
        let body = render_digest_body(&params, &[plain, starred]);
        assert!(body.contains("[[b|Starred]]"), "{body}");
        assert!(body.contains("Quiet items"), "{body}");
        assert!(body.contains("[[a|Plain]]"), "{body}");
    }

    #[test]
    fn extractive_summary_skips_machinery() {
        let body = "<!-- curio:managed:begin v1 -->\n# Title\n\nFirst real paragraph\nwith a second line.\n\nSecond paragraph.\n<!-- curio:managed:end -->\n";
        assert_eq!(
            extractive_summary(body, 160),
            "First real paragraph with a second line."
        );
        // Fences are skipped whole.
        let body = "```rust\nlet x = 1;\n```\n\nProse after the fence.\n";
        assert_eq!(extractive_summary(body, 160), "Prose after the fence.");
        // Clipping is char-safe and flagged.
        assert_eq!(extractive_summary("abcdef\n", 3), "abc…");
        // Nothing but machinery = empty.
        assert_eq!(extractive_summary("# Only\n## Headings\n", 160), "");
    }

    #[test]
    fn cluster_key_prefers_tags_then_source_host() {
        let mut c = candidate("kp:x", "x.md", "X");
        assert_eq!(cluster_key(&c), "notes");
        c.source = Some("https://example.com/article".to_owned());
        assert_eq!(cluster_key(&c), "example.com");
        c.tags = vec!["rust".to_owned()];
        assert_eq!(cluster_key(&c), "rust");
    }
}
