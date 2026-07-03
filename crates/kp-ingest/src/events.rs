//! The Curio events tail — the consumer side of `curio.events.v1`.
//!
//! Curio appends behavioral events to per-profile JSONL files
//! (`events-YYYYMMDD.jsonl`, size-rotated with `-2`/`-3` suffixes, never
//! rewritten, retained ≥ 90 days). This tail folds them into the
//! `behavior` rollup table, and nothing else — raw behavioral history is
//! never committed to git and never stored beyond the aggregates.
//!
//! Consumer discipline (from the contract + kp-note/v1):
//! - **cursors**: one `(file, line)` row per physical file in the index's
//!   `cursors` table; a resume reads each file from its cursor;
//! - **rotation/retention**: files are processed in `(date, seq)` order;
//!   a file that vanished (producer retention) gets its cursor dropped;
//!   a file without a cursor is read from the top — which is exactly the
//!   "restart from the oldest existing file" rule;
//! - **dedupe**: every event id is recorded per consumer; replays (seen
//!   ids) never fold twice, which is what makes restarts safe. Seen ids
//!   older than the oldest existing file's date are pruned — events from
//!   vanished files can never be replayed;
//! - **negation**: folding honors negation events (unstar, read-later
//!   removal) — histories are not monotone;
//! - **malformed lines**: warn + skip, never crash; the cursor still
//!   advances past them;
//! - **torn tail**: an unterminated final line (the producer caught
//!   mid-append) is left for the next pass — the cursor only advances
//!   past newline-terminated lines, so a half-written event is never
//!   consumed (and never lost once its newline lands).

use std::collections::BTreeSet;
use std::path::Path;

use kp_index::{BehaviorDelta, Index};

use crate::curio::{CurioAdapter, CurioEvent};
use crate::error::IngestError;

/// The cursor/dedupe namespace this consumer owns in the index.
pub const EVENTS_CONSUMER: &str = "curio-events";

/// What one tail pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TailReport {
    /// Event files seen (in rotation order).
    pub files: usize,
    /// Events consumed and folded (first sighting).
    pub folded: usize,
    /// Events skipped as replays (event id already seen).
    pub duplicates: usize,
    /// Lines skipped as malformed (bad JSON or schema violation).
    pub malformed: usize,
    /// Cursor rows dropped because their file vanished (retention).
    pub cursors_removed: usize,
    /// Seen-event ids pruned (older than the oldest existing file).
    pub seen_ids_pruned: usize,
}

/// Tail every event file in `events_dir` from its cursor, folding new
/// events into the behavior table. A missing directory is an empty tail.
pub fn tail_events(
    events_dir: &Path,
    adapter: &CurioAdapter,
    index: &mut Index,
) -> Result<TailReport, IngestError> {
    let mut report = TailReport::default();
    let files = event_files(events_dir)?;

    // Housekeeping: drop cursors whose files retention has deleted. Their
    // events can never be re-read; the seen-id prune below retires the
    // matching dedupe rows.
    let existing: BTreeSet<&str> = files.iter().map(|(_, name)| name.as_str()).collect();
    for (file, _) in index.cursors_for(EVENTS_CONSUMER)? {
        if !existing.contains(file.as_str()) {
            index.remove_cursor(EVENTS_CONSUMER, &file)?;
            report.cursors_removed += 1;
        }
    }

    for (_, name) in &files {
        report.files += 1;
        let path = events_dir.join(name);
        let content = std::fs::read_to_string(&path).map_err(|source| IngestError::Io {
            path: path.clone(),
            source,
        })?;
        let start = index.cursor(EVENTS_CONSUMER, name)?.unwrap_or(0);
        // A JSONL producer appends one `line\n` per event, but a read can
        // still catch the final line mid-write (no trailing newline yet).
        // That torn fragment is NOT consumed and the cursor never advances
        // past it — otherwise the completed event would be skipped forever
        // on the next pass (it would sit at a line number <= the cursor).
        let terminated = if content.ends_with('\n') {
            content.lines().count()
        } else {
            content.lines().count().saturating_sub(1)
        };
        let mut line_no: i64 = 0;
        for line in content.lines().take(terminated) {
            line_no += 1;
            if line_no <= start {
                continue;
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match adapter.parse_event(line) {
                Err(warning) => {
                    tracing::warn!(file = %name, line = line_no, %warning,
                        "skipping malformed event line");
                    report.malformed += 1;
                }
                Ok(event) => {
                    // Seen-mark + fold in ONE transaction (dedupe by
                    // event_id): a crash between the two can never leave
                    // an event marked seen but unfolded — which every
                    // retry would then skip as a duplicate.
                    let delta = fold(&event);
                    let newly = index.fold_event(
                        EVENTS_CONSUMER,
                        &event.event_id,
                        &event.ts,
                        delta.as_ref().map(|(kp_id, d)| (kp_id.as_str(), d)),
                    )?;
                    if newly {
                        report.folded += 1;
                    } else {
                        report.duplicates += 1;
                    }
                }
            }
        }
        index.set_cursor(EVENTS_CONSUMER, name, line_no.max(start))?;
    }

    // Prune seen ids that predate the oldest existing file: those events
    // now live nowhere, so they can never be replayed.
    if let Some(((date, _), _)) = files.first().map(|(k, n)| (k.clone(), n)) {
        let horizon = format!(
            "{}-{}-{}T00:00:00.000Z",
            &date[0..4],
            &date[4..6],
            &date[6..8]
        );
        report.seen_ids_pruned = index.prune_seen_events(EVENTS_CONSUMER, &horizon)?;
    }
    Ok(report)
}

/// `((date, seq), file_name)` — the rotation sort key plus the name.
type EventFileEntry = ((String, u32), String);

/// Event files in `(date, seq)` rotation order. Names outside the
/// `events-YYYYMMDD[-N].jsonl` shape are not ours and are ignored.
fn event_files(dir: &Path) -> Result<Vec<EventFileEntry>, IngestError> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(source) => {
            return Err(IngestError::Io {
                path: dir.to_owned(),
                source,
            });
        }
    };
    for entry in entries {
        let entry = entry.map_err(|source| IngestError::Io {
            path: dir.to_owned(),
            source,
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(key) = event_file_key(&name) {
            out.push((key, name));
        }
    }
    // Lexicographic (date, seq) IS rotation order — plain name sorting is
    // not ("-2" sorts before "."), which is why the parsed key exists.
    out.sort();
    Ok(out)
}

/// Parse `events-YYYYMMDD.jsonl` / `events-YYYYMMDD-N.jsonl` into a
/// `(date, seq)` sort key (base file = seq 1; overflow files start at 2).
fn event_file_key(name: &str) -> Option<(String, u32)> {
    let rest = name.strip_prefix("events-")?.strip_suffix(".jsonl")?;
    let (date, seq) = match rest.split_once('-') {
        Some((date, seq)) => (date, seq.parse::<u32>().ok()?),
        None => (rest, 1),
    };
    if date.len() != 8 || !date.bytes().all(|b| b.is_ascii_digit()) || seq == 0 {
        return None;
    }
    Some((date.to_owned(), seq))
}

/// Map one event onto its behavioral delta. `None` for events with no
/// behavioral rollup (feed.* — no article identity).
fn fold(event: &CurioEvent) -> Option<(String, BehaviorDelta)> {
    let curio_id = event.curio_id()?;
    let kp_id = format!("curio:{curio_id}");
    let mut delta = BehaviorDelta {
        activity_ts: Some(event.ts.clone()),
        ..Default::default()
    };
    match event.kind.as_str() {
        "article.opened" => delta.opened_delta = 1,
        "article.starred" => delta.starred = Some(true),
        // NEGATION events: membership is removed, not accumulated.
        "article.unstarred" => delta.starred = Some(false),
        "article.read_later.added" => delta.read_later = Some(true),
        "article.read_later.removed" => delta.read_later = Some(false),
        // saved/updated/archived/tagged/... : activity signal only.
        _ => {}
    }
    Some((kp_id, delta))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kp_index::{BehaviorStats, HashEmbedder};
    use serde_json::json;

    const ID_A: &str = "0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d";

    /// Deterministic valid ULIDs (Crockford base32, first char 0).
    fn ulid(n: usize) -> String {
        const ALPHA: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
        format!(
            "01ARZ3NDEKTSV4RRFFQ69G5F{}{}",
            ALPHA[(n / 32) % 32] as char,
            ALPHA[n % 32] as char
        )
    }

    fn ev(n: usize, ts: &str, kind: &str, payload: serde_json::Value) -> String {
        json!({
            "schema": "curio.events.v1",
            "event_id": ulid(n),
            "ts": ts,
            "type": kind,
            "payload": payload,
        })
        .to_string()
    }

    fn id_only(id: &str) -> serde_json::Value {
        json!({ "curio_id": id })
    }

    fn with_tags(id: &str) -> serde_json::Value {
        json!({ "curio_id": id, "tags": ["rust"] })
    }

    fn saved_snapshot(id: &str) -> serde_json::Value {
        json!({
            "curio_id": id,
            "title": "Async patterns",
            "source": "https://example.com/async",
            "feed": "https://example.com/feed.xml",
            "feed_title": "Example Blog",
            "tags": ["rust", "async"],
            "published": "2026-07-01T12:00:00Z",
            "destination": "vault",
            "path": "curio/async.md",
            "checksum": format!("sha256:{}", "9".repeat(64)),
        })
    }

    /// One step of a scenario: mutate the events dir, run the tail, check.
    struct Step {
        write: Vec<(&'static str, String)>,
        remove: Vec<&'static str>,
        expect_report: TailReport,
        expect_behavior: Vec<(&'static str, BehaviorStats)>,
    }

    fn run_scenario(name: &str, steps: Vec<Step>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let events = dir.path().join("events");
        std::fs::create_dir_all(&events).expect("mkdir");
        let embedder = HashEmbedder::new(16);
        let mut index = Index::create(dir.path().join("index.db"), &embedder, 1).expect("create");
        let adapter = CurioAdapter::new();

        for (i, step) in steps.into_iter().enumerate() {
            for (file, content) in &step.write {
                // Contract-shaped JSONL: every event line is terminated.
                // (The torn-tail test writes its file directly.)
                let mut content = content.clone();
                if !content.is_empty() && !content.ends_with('\n') {
                    content.push('\n');
                }
                std::fs::write(events.join(file), content).expect("write");
            }
            for file in &step.remove {
                std::fs::remove_file(events.join(file)).expect("remove");
            }
            let report = tail_events(&events, &adapter, &mut index).expect("tail");
            assert_eq!(report, step.expect_report, "{name}: report of step {i}");
            for (kp_id, want) in &step.expect_behavior {
                let got = index
                    .behavior(kp_id)
                    .expect("query")
                    .unwrap_or_else(|| panic!("{name}: step {i}: no behavior row for {kp_id}"));
                assert_eq!(got, *want, "{name}: behavior of {kp_id} at step {i}");
            }
        }
    }

    fn report(
        files: usize,
        folded: usize,
        duplicates: usize,
        malformed: usize,
        cursors_removed: usize,
        seen_ids_pruned: usize,
    ) -> TailReport {
        TailReport {
            files,
            folded,
            duplicates,
            malformed,
            cursors_removed,
            seen_ids_pruned,
        }
    }

    fn stats(opened: i64, starred: bool, read_later: bool, last_activity: &str) -> BehaviorStats {
        BehaviorStats {
            opened_count: opened,
            starred,
            read_later,
            last_activity: Some(last_activity.to_owned()),
        }
    }

    #[test]
    fn rotation_across_files_folds_in_order() {
        // Order matters: the star lands in the base file, the negation in
        // the size-overflow file (-2), the open on the next day. Only the
        // correct (date, seq) order ends unstarred.
        run_scenario(
            "rotation",
            vec![Step {
                write: vec![
                    (
                        "events-20260701.jsonl",
                        [
                            ev(
                                1,
                                "2026-07-01T09:00:00.000Z",
                                "article.saved",
                                saved_snapshot(ID_A),
                            ),
                            ev(
                                2,
                                "2026-07-01T09:05:00.000Z",
                                "article.starred",
                                with_tags(ID_A),
                            ),
                        ]
                        .join("\n"),
                    ),
                    (
                        "events-20260701-2.jsonl",
                        ev(
                            3,
                            "2026-07-01T21:00:00.000Z",
                            "article.unstarred",
                            id_only(ID_A),
                        ),
                    ),
                    (
                        "events-20260702.jsonl",
                        ev(
                            4,
                            "2026-07-02T08:00:00.000Z",
                            "article.opened",
                            id_only(ID_A),
                        ),
                    ),
                ],
                remove: vec![],
                expect_report: report(3, 4, 0, 0, 0, 0),
                expect_behavior: vec![(
                    "curio:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d",
                    stats(1, false, false, "2026-07-02T08:00:00.000Z"),
                )],
            }],
        );
    }

    #[test]
    fn negations_fold_within_one_file() {
        run_scenario(
            "negation",
            vec![Step {
                write: vec![(
                    "events-20260701.jsonl",
                    [
                        ev(
                            1,
                            "2026-07-01T09:00:00.000Z",
                            "article.starred",
                            with_tags(ID_A),
                        ),
                        ev(
                            2,
                            "2026-07-01T09:01:00.000Z",
                            "article.read_later.added",
                            with_tags(ID_A),
                        ),
                        ev(
                            3,
                            "2026-07-01T09:02:00.000Z",
                            "article.unstarred",
                            id_only(ID_A),
                        ),
                        ev(
                            4,
                            "2026-07-01T09:03:00.000Z",
                            "article.read_later.removed",
                            id_only(ID_A),
                        ),
                        ev(
                            5,
                            "2026-07-01T09:04:00.000Z",
                            "article.opened",
                            id_only(ID_A),
                        ),
                    ]
                    .join("\n"),
                )],
                remove: vec![],
                expect_report: report(1, 5, 0, 0, 0, 0),
                expect_behavior: vec![(
                    "curio:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d",
                    stats(1, false, false, "2026-07-01T09:04:00.000Z"),
                )],
            }],
        );
    }

    #[test]
    fn cursor_resumes_mid_file() {
        let first = [
            ev(
                1,
                "2026-07-01T09:00:00.000Z",
                "article.saved",
                saved_snapshot(ID_A),
            ),
            ev(
                2,
                "2026-07-01T09:05:00.000Z",
                "article.opened",
                id_only(ID_A),
            ),
        ]
        .join("\n");
        let extended = [
            first.clone(),
            ev(
                3,
                "2026-07-01T10:00:00.000Z",
                "article.opened",
                id_only(ID_A),
            ),
            ev(
                4,
                "2026-07-01T11:00:00.000Z",
                "article.opened",
                id_only(ID_A),
            ),
        ]
        .join("\n");
        run_scenario(
            "resume",
            vec![
                Step {
                    write: vec![("events-20260701.jsonl", first)],
                    remove: vec![],
                    expect_report: report(1, 2, 0, 0, 0, 0),
                    expect_behavior: vec![(
                        "curio:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d",
                        stats(1, false, false, "2026-07-01T09:05:00.000Z"),
                    )],
                },
                // The producer appended two lines; only those fold — zero
                // duplicates proves the CURSOR (not dedupe) did the resume.
                Step {
                    write: vec![("events-20260701.jsonl", extended)],
                    remove: vec![],
                    expect_report: report(1, 2, 0, 0, 0, 0),
                    expect_behavior: vec![(
                        "curio:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d",
                        stats(3, false, false, "2026-07-01T11:00:00.000Z"),
                    )],
                },
            ],
        );
    }

    #[test]
    fn deleted_oldest_file_restarts_and_dedupes() {
        // f2 replays event 2 (rotation overlap) and adds event 3. After f1
        // vanishes, f2 has no cursor → read from the top; the replay is
        // caught by event-id dedupe, the new event folds.
        run_scenario(
            "retention-restart",
            vec![
                Step {
                    write: vec![(
                        "events-20260701.jsonl",
                        [
                            ev(
                                1,
                                "2026-07-01T09:00:00.000Z",
                                "article.saved",
                                saved_snapshot(ID_A),
                            ),
                            ev(
                                2,
                                "2026-07-01T09:05:00.000Z",
                                "article.opened",
                                id_only(ID_A),
                            ),
                        ]
                        .join("\n"),
                    )],
                    remove: vec![],
                    expect_report: report(1, 2, 0, 0, 0, 0),
                    expect_behavior: vec![],
                },
                Step {
                    write: vec![(
                        "events-20260702.jsonl",
                        [
                            ev(
                                2,
                                "2026-07-01T09:05:00.000Z",
                                "article.opened",
                                id_only(ID_A),
                            ),
                            ev(
                                3,
                                "2026-07-02T09:00:00.000Z",
                                "article.starred",
                                with_tags(ID_A),
                            ),
                        ]
                        .join("\n"),
                    )],
                    remove: vec!["events-20260701.jsonl"],
                    // The stale cursor is dropped; the seen ids for July 1
                    // (events 1 and 2) are pruned AFTER this pass because
                    // the oldest existing file is now July 2.
                    expect_report: report(1, 1, 1, 0, 1, 2),
                    expect_behavior: vec![(
                        "curio:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d",
                        stats(1, true, false, "2026-07-02T09:00:00.000Z"),
                    )],
                },
            ],
        );
    }

    #[test]
    fn malformed_lines_warn_skip_and_never_refold() {
        let content = [
            ev(
                1,
                "2026-07-01T09:00:00.000Z",
                "article.opened",
                id_only(ID_A),
            ),
            "{this is not json".to_owned(),
            // Valid JSON, invalid schema (unknown type).
            ev(
                2,
                "2026-07-01T09:01:00.000Z",
                "article.exploded",
                id_only(ID_A),
            ),
            ev(
                3,
                "2026-07-01T09:02:00.000Z",
                "article.starred",
                with_tags(ID_A),
            ),
        ]
        .join("\n");
        run_scenario(
            "malformed",
            vec![
                Step {
                    write: vec![("events-20260701.jsonl", content.clone())],
                    remove: vec![],
                    expect_report: report(1, 2, 0, 2, 0, 0),
                    expect_behavior: vec![(
                        "curio:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d",
                        stats(1, true, false, "2026-07-01T09:02:00.000Z"),
                    )],
                },
                // Second pass: the cursor is PAST the malformed lines —
                // nothing is re-read, re-warned, or re-folded.
                Step {
                    write: vec![],
                    remove: vec![],
                    expect_report: report(1, 0, 0, 0, 0, 0),
                    expect_behavior: vec![(
                        "curio:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d",
                        stats(1, true, false, "2026-07-01T09:02:00.000Z"),
                    )],
                },
            ],
        );
    }

    /// Regression: a read that catches the producer mid-append (an
    /// unterminated final line) must neither count the fragment as
    /// malformed nor advance the cursor past it — once the newline lands
    /// the completed event still folds.
    #[test]
    fn torn_final_line_is_left_for_the_next_pass() {
        let dir = tempfile::tempdir().expect("tempdir");
        let events = dir.path().join("events");
        std::fs::create_dir_all(&events).expect("mkdir");
        let embedder = HashEmbedder::new(16);
        let mut index = Index::create(dir.path().join("index.db"), &embedder, 1).expect("create");
        let adapter = CurioAdapter::new();
        let file = events.join("events-20260701.jsonl");

        let complete = ev(
            1,
            "2026-07-01T09:00:00.000Z",
            "article.opened",
            id_only(ID_A),
        );
        let second = ev(
            2,
            "2026-07-01T09:05:00.000Z",
            "article.starred",
            with_tags(ID_A),
        );
        // The producer has written event 1 fully and HALF of event 2.
        let torn = &second[..second.len() / 2];
        std::fs::write(&file, format!("{complete}\n{torn}")).expect("write torn");

        let report = tail_events(&events, &adapter, &mut index).expect("tail");
        assert_eq!(
            report,
            super::TailReport {
                files: 1,
                folded: 1,
                ..Default::default()
            },
            "the torn fragment is neither folded nor counted malformed"
        );
        assert_eq!(
            index
                .cursor(EVENTS_CONSUMER, "events-20260701.jsonl")
                .expect("cursor"),
            Some(1),
            "the cursor must stop at the last terminated line"
        );

        // The producer finishes the append; the completed event folds.
        std::fs::write(&file, format!("{complete}\n{second}\n")).expect("complete");
        let report = tail_events(&events, &adapter, &mut index).expect("tail");
        assert_eq!(report.folded, 1, "the completed event is not lost");
        assert_eq!(report.duplicates, 0);
        assert_eq!(report.malformed, 0);
        let stats = index
            .behavior("curio:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d")
            .expect("query")
            .expect("row");
        assert!(stats.starred, "the once-torn star event folded");
        assert_eq!(stats.opened_count, 1);
    }

    #[test]
    fn missing_dir_and_foreign_files_are_ignored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let embedder = HashEmbedder::new(16);
        let mut index = Index::create(dir.path().join("index.db"), &embedder, 1).expect("create");
        let adapter = CurioAdapter::new();
        // Directory does not exist: an empty tail, not an error.
        let report = tail_events(&dir.path().join("nope"), &adapter, &mut index).expect("tail");
        assert_eq!(report, TailReport::default());
        // Foreign file names are not event files.
        let events = dir.path().join("events");
        std::fs::create_dir_all(&events).expect("mkdir");
        std::fs::write(events.join("notes.txt"), "x").expect("write");
        std::fs::write(events.join("events-notadate.jsonl"), "x").expect("write");
        std::fs::write(events.join("events-20260701.txt"), "x").expect("write");
        let report = tail_events(&events, &adapter, &mut index).expect("tail");
        assert_eq!(report, TailReport::default());
    }

    #[test]
    fn rotation_sort_key_orders_base_before_overflow() {
        assert_eq!(
            event_file_key("events-20260701.jsonl"),
            Some(("20260701".to_owned(), 1))
        );
        assert_eq!(
            event_file_key("events-20260701-2.jsonl"),
            Some(("20260701".to_owned(), 2))
        );
        assert_eq!(event_file_key("events-20260701-0.jsonl"), None);
        assert_eq!(event_file_key("events-2026071.jsonl"), None);
        assert_eq!(event_file_key("other.jsonl"), None);
        // Plain lexicographic name order would put "-2" FIRST; the key
        // must not.
        let mut keys = [
            (event_file_key("events-20260701-2.jsonl").expect("key"), "a"),
            (event_file_key("events-20260701.jsonl").expect("key"), "b"),
        ];
        keys.sort();
        assert_eq!(keys[0].1, "b", "base file must sort before overflow");
    }
}
