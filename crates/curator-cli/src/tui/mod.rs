//! `curator review` — the interactive terminal UI.
//!
//! A tabbed shell over three screens: **Review** (the `proposals/v1` queue —
//! diff, pre-flight drift check, apply/reject), **Search** (interactive
//! hybrid retrieval over the same `KpEngine` the MCP surface rides), and
//! **Digest** (a read-only preview of what the deterministic librarian would
//! surface, with one-key generate).
//!
//! Every screen is a pure reducer ([`app`], [`search`], [`digest`]) with no
//! terminal and no I/O. This file is the only effectful layer: it owns the
//! terminal, decodes key events into each screen's message alphabet, and runs
//! the returned actions against the vault / engine / librarian. The index and
//! embedder are built **lazily** — a review-only session never opens them, so
//! `curator review` stays as fast to start as it was before Search and Digest
//! existed.

mod app;
mod common;
mod diff;
mod digest;
mod search;
mod shell;
mod view;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use curator_core::{KpConfig, Proposal, ProposalStatus, Vault};
use curator_index::{Embedder, embedder_from_config};
use curator_librarian::{ApplyError, DigestPreview, FilePatch, RejectError};
use curator_mcp::KpEngine;
use curator_mcp::types::{HitOutput, NoteOutput, SearchMode};

use app::{Loaded, Preflight, ReviewApp, short_id};
use common::Flash;
use digest::DigestApp;
use shell::{GlobalMsg, Shell, Tab};

/// Result rows fetched per interactive search.
const SEARCH_K: u32 = 20;
/// Body characters shown in a digest candidate preview.
const DIGEST_PREVIEW_CHARS: usize = 240;

/// Launch the reviewer over the vault's proposal queue. Returns when the user
/// quits (or on a terminal I/O error). The terminal is always restored —
/// `ratatui::init` installs a panic hook that restores it too, so a panic
/// mid-draw won't wedge the user's shell.
pub fn run_review(config: &KpConfig) -> Result<(), String> {
    let vault = Vault::open(config.vault_path()).map_err(|e| e.to_string())?;
    let proposals_dir = config.vault.proposals_dir.clone();
    let proposals =
        curator_core::list_proposals(&vault, &proposals_dir).map_err(|e| e.to_string())?;
    let mut shell = Shell::new(ReviewApp::new(proposals));

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut shell, config, &vault, &proposals_dir);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    shell: &mut Shell,
    config: &KpConfig,
    vault: &Vault,
    proposals_dir: &str,
) -> Result<(), String> {
    // Review detail (parsed patch + pre-flight), reloaded when the selection
    // changes — held here, outside the pure reducer.
    let mut loaded: Option<Loaded> = None;
    let mut loaded_id: Option<String> = None;

    // Lazily-built index-backed resources: only opened on first use.
    let mut engine: Option<KpEngine> = None;
    let mut embedder: Option<Box<dyn Embedder>> = None;

    loop {
        // Refresh the Review detail pane whenever its selection changes.
        let sel = shell.review.selected_id().map(str::to_owned);
        if sel != loaded_id {
            loaded = match sel.as_deref() {
                Some(id) => match load_detail(vault, proposals_dir, id) {
                    Ok(l) => Some(l),
                    Err(e) => {
                        shell.review.set_flash(Flash::warn(e));
                        None
                    }
                },
                None => None,
            };
            loaded_id = sel;
        }

        terminal
            .draw(|f| shell::render(f, shell, loaded.as_ref()))
            .map_err(|e| e.to_string())?;

        let Event::Key(key) = event::read().map_err(|e| e.to_string())? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue; // ignore key-release / repeat on platforms that send them
        }

        // The help overlay swallows the next key press.
        if shell.help {
            shell.help = false;
            continue;
        }

        // Global routing first (tab switch / quit / help).
        if let Some(g) = shell::decode_global(key, shell.active, shell.active_is_modal()) {
            match g {
                GlobalMsg::Quit => break,
                GlobalMsg::ToggleHelp => shell.help = true,
                GlobalMsg::Switch(tab) => {
                    // Leaving a screen abandons any armed confirm overlay, so a
                    // stale apply/reject/generate can't be confirmed on return.
                    shell.review.cancel_confirm();
                    shell.digest.cancel_confirm();
                    shell.active = tab;
                    if tab == Tab::Digest && !shell.digest.is_loaded() {
                        load_digest(shell, &mut embedder, config);
                    }
                }
            }
            continue;
        }

        // Otherwise the key belongs to the active screen.
        match shell.active {
            Tab::Review => handle_review(key, shell, vault, proposals_dir, &mut loaded_id)?,
            Tab::Search => handle_search(key, shell, config, &mut engine),
            Tab::Digest => handle_digest(
                key,
                shell,
                config,
                vault,
                proposals_dir,
                &mut embedder,
                &mut loaded_id,
            )?,
        }
    }
    Ok(())
}

// --- Review screen effects ---

fn handle_review(
    key: KeyEvent,
    shell: &mut Shell,
    vault: &Vault,
    proposals_dir: &str,
    loaded_id: &mut Option<String>,
) -> Result<(), String> {
    let Some(msg) = decode_review(key, shell.review.is_modal()) else {
        return Ok(());
    };
    match shell.review.update(msg) {
        app::Action::None => {}
        app::Action::Reload => {
            reload_review(&mut shell.review, vault, proposals_dir)?;
            *loaded_id = None;
        }
        app::Action::Apply(id) => {
            let flash = match curator_librarian::apply_proposal(vault, proposals_dir, &id) {
                Ok(report) => Flash::success(format!(
                    "applied {} — {} file(s) written",
                    short_id(&report.id),
                    report.files_written.len()
                )),
                Err(e) => apply_error_flash(&e),
            };
            shell.review.set_flash(flash);
            reload_review(&mut shell.review, vault, proposals_dir)?;
            *loaded_id = None;
        }
        app::Action::Reject(id) => {
            let flash = match curator_librarian::reject_proposal(vault, proposals_dir, &id) {
                Ok(p) => Flash::success(format!("rejected {} — {}", short_id(&p.id), p.title)),
                Err(e) => reject_error_flash(&e),
            };
            shell.review.set_flash(flash);
            reload_review(&mut shell.review, vault, proposals_dir)?;
            *loaded_id = None;
        }
    }
    Ok(())
}

fn reload_review(review: &mut ReviewApp, vault: &Vault, proposals_dir: &str) -> Result<(), String> {
    let proposals =
        curator_core::list_proposals(vault, proposals_dir).map_err(|e| e.to_string())?;
    review.reload(proposals);
    Ok(())
}

// --- Search screen effects ---

fn handle_search(
    key: KeyEvent,
    shell: &mut Shell,
    config: &KpConfig,
    engine: &mut Option<KpEngine>,
) {
    let Some(msg) = decode_search(key, shell.search.is_typing()) else {
        return;
    };
    match shell.search.update(msg) {
        search::Action::None => {}
        search::Action::Search(query, mode) => match lazy_engine(engine, config) {
            Ok(eng) => match eng.search(&query, Some(SEARCH_K), Some(to_search_mode(mode))) {
                Ok(out) => {
                    let n = out.results.len();
                    let status = if n == 0 {
                        format!("no hits · {}", mode.label())
                    } else {
                        format!("{n} hits · {}", mode.label())
                    };
                    shell.search.set_results(map_hits(out.results), status);
                }
                Err(e) => shell
                    .search
                    .set_flash(Flash::error(format!("search failed: {e}"))),
            },
            Err(e) => shell
                .search
                .set_flash(Flash::error(format!("index unavailable: {e}"))),
        },
        search::Action::Open(id) => match lazy_engine(engine, config) {
            Ok(eng) => match eng.get_note(&id) {
                Ok(note) => shell.search.set_opened(map_note(note)),
                Err(e) => shell
                    .search
                    .set_flash(Flash::error(format!("open failed: {e}"))),
            },
            Err(e) => shell
                .search
                .set_flash(Flash::error(format!("index unavailable: {e}"))),
        },
        search::Action::Related(id) => match lazy_engine(engine, config) {
            Ok(eng) => match eng.related(&id, Some(SEARCH_K)) {
                Ok(out) => {
                    let n = out.results.len();
                    let status = format!("related · {n} note(s)");
                    shell.search.set_results(map_hits(out.results), status);
                }
                Err(e) => shell
                    .search
                    .set_flash(Flash::error(format!("related failed: {e}"))),
            },
            Err(e) => shell
                .search
                .set_flash(Flash::error(format!("index unavailable: {e}"))),
        },
    }
}

// --- Digest screen effects ---

fn handle_digest(
    key: KeyEvent,
    shell: &mut Shell,
    config: &KpConfig,
    vault: &Vault,
    proposals_dir: &str,
    embedder: &mut Option<Box<dyn Embedder>>,
    loaded_id: &mut Option<String>,
) -> Result<(), String> {
    let Some(msg) = decode_digest(key, shell.digest.is_modal()) else {
        return Ok(());
    };
    match shell.digest.update(msg) {
        digest::Action::None => {}
        digest::Action::Reload => load_digest(shell, embedder, config),
        digest::Action::Generate => {
            generate_digest(shell, config, vault, proposals_dir, embedder, loaded_id)?;
        }
    }
    Ok(())
}

/// Build (or refresh) the digest preview from the index.
fn load_digest(shell: &mut Shell, embedder: &mut Option<Box<dyn Embedder>>, config: &KpConfig) {
    let now = curator_core::time::unix_now();
    match lazy_embedder(embedder, config) {
        Ok(emb) => match curator_librarian::preview_digest(config, emb, now) {
            Ok(preview) => shell.digest.set_preview(map_preview(preview)),
            Err(e) => shell
                .digest
                .set_flash(Flash::error(format!("digest preview failed: {e}"))),
        },
        Err(e) => shell
            .digest
            .set_flash(Flash::error(format!("index unavailable: {e}"))),
    }
}

/// File today's digest as a proposal, then jump to the Review tab to review it.
fn generate_digest(
    shell: &mut Shell,
    config: &KpConfig,
    vault: &Vault,
    proposals_dir: &str,
    embedder: &mut Option<Box<dyn Embedder>>,
    loaded_id: &mut Option<String>,
) -> Result<(), String> {
    let now = curator_core::time::unix_now();
    let outcome = match lazy_embedder(embedder, config) {
        Ok(emb) => curator_librarian::run_digest(config, emb, now, false)
            .map_err(|e| format!("generate failed: {e}")),
        Err(e) => Err(format!("index unavailable: {e}")),
    };
    match outcome {
        Err(e) => shell.digest.set_flash(Flash::error(e)),
        Ok(report) => {
            if let Some(reason) = report.skipped {
                shell.digest.set_flash(Flash::warn(reason));
            } else {
                reload_review(&mut shell.review, vault, proposals_dir)?;
                let pid = report.proposal_id.as_deref().unwrap_or("");
                // Jump to the freshly-filed proposal: it sorts LAST by ULID,
                // so seek it by id rather than leaving the old selection. Also
                // clear any stale Review confirm before landing there.
                shell.review.cancel_confirm();
                shell.review.select_by_id(pid);
                shell.review.set_flash(Flash::success(format!(
                    "digest filed as proposal {} — review it",
                    short_id(pid)
                )));
                shell.active = Tab::Review;
                *loaded_id = None;
                // The preview is stale (a digest now exists) — drop it so the
                // next Digest visit reloads and reports already-generated.
                shell.digest = DigestApp::default();
            }
        }
    }
    Ok(())
}

// --- lazy index-backed resources ---

fn lazy_engine<'a>(
    cache: &'a mut Option<KpEngine>,
    config: &KpConfig,
) -> Result<&'a KpEngine, String> {
    if cache.is_none() {
        *cache = Some(KpEngine::from_config(config.clone()).map_err(|e| e.to_string())?);
    }
    Ok(cache.as_ref().expect("just populated"))
}

fn lazy_embedder<'a>(
    cache: &'a mut Option<Box<dyn Embedder>>,
    config: &KpConfig,
) -> Result<&'a dyn Embedder, String> {
    if cache.is_none() {
        *cache = Some(embedder_from_config(config).map_err(|e| e.to_string())?);
    }
    Ok(cache.as_deref().expect("just populated"))
}

// --- engine/librarian → screen type mappings ---

fn to_search_mode(mode: search::Mode) -> SearchMode {
    match mode {
        search::Mode::Hybrid => SearchMode::Hybrid,
        search::Mode::Vector => SearchMode::Vector,
        search::Mode::Fts => SearchMode::Fts,
    }
}

fn map_hits(hits: Vec<HitOutput>) -> Vec<search::Hit> {
    hits.into_iter()
        .map(|h| search::Hit {
            score: h.score,
            id: h.id,
            title: h.title,
            path: h.path,
            snippet: h.snippet,
        })
        .collect()
}

fn map_note(note: NoteOutput) -> search::OpenedNote {
    search::OpenedNote {
        id: note.id,
        title: note.title,
        path: note.path,
        tags: note.frontmatter.tags,
        source: note.frontmatter.source,
        ingested_at: note.index.ingested_at,
        content: note.content,
    }
}

fn map_preview(preview: DigestPreview) -> digest::Preview {
    let DigestPreview {
        date,
        note_path,
        candidates,
        ranked,
        surfaced,
        quiet,
        warnings,
        already_exists,
        ..
    } = preview;
    let rows = ranked
        .into_iter()
        .filter_map(|r| {
            let c = candidates.get(r.index)?;
            Some(digest::Row {
                title: c.title.clone(),
                path: c.path.clone(),
                tags: c.tags.clone(),
                source: c.source.clone(),
                score: r.score,
                similarity: r.similarity,
                age_days: r.age_days,
                why: r.why,
                surfaced: r.surfaced,
                preview: curator_librarian::extractive_summary(&c.body, DIGEST_PREVIEW_CHARS),
            })
        })
        .collect();
    digest::Preview {
        date,
        note_path,
        rows,
        surfaced,
        quiet,
        warnings,
        already_exists,
    }
}

// --- key decoders (raw key → each screen's Msg) ---

fn decode_review(key: KeyEvent, confirm: bool) -> Option<app::Msg> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if confirm {
        return match key.code {
            KeyCode::Char('y' | 'Y') | KeyCode::Enter => Some(app::Msg::Confirm),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(app::Msg::Cancel),
            _ => None,
        };
    }
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => Some(app::Msg::Down),
        KeyCode::Char('k') | KeyCode::Up => Some(app::Msg::Up),
        KeyCode::Char('d') if ctrl => Some(app::Msg::ScrollDown),
        KeyCode::Char('u') if ctrl => Some(app::Msg::ScrollUp),
        KeyCode::PageDown => Some(app::Msg::ScrollDown),
        KeyCode::PageUp => Some(app::Msg::ScrollUp),
        KeyCode::Char('f') => Some(app::Msg::CycleFilter),
        KeyCode::Char('a') => Some(app::Msg::RequestApply),
        KeyCode::Char('x') => Some(app::Msg::RequestReject),
        KeyCode::Char('r') => Some(app::Msg::Refresh),
        _ => None,
    }
}

fn decode_search(key: KeyEvent, typing: bool) -> Option<search::Msg> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if typing {
        return match key.code {
            KeyCode::Char(c) if !ctrl => Some(search::Msg::Char(c)),
            KeyCode::Backspace => Some(search::Msg::Backspace),
            KeyCode::Enter => Some(search::Msg::Submit),
            KeyCode::Esc => Some(search::Msg::Blur),
            _ => None,
        };
    }
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => Some(search::Msg::Down),
        KeyCode::Char('k') | KeyCode::Up => Some(search::Msg::Up),
        KeyCode::Char('d') if ctrl => Some(search::Msg::ScrollDown),
        KeyCode::Char('u') if ctrl => Some(search::Msg::ScrollUp),
        KeyCode::PageDown => Some(search::Msg::ScrollDown),
        KeyCode::PageUp => Some(search::Msg::ScrollUp),
        KeyCode::Char('o') | KeyCode::Enter => Some(search::Msg::Open),
        KeyCode::Char('r') => Some(search::Msg::Related),
        KeyCode::Char('m') => Some(search::Msg::CycleMode),
        KeyCode::Char('/' | 'i') => Some(search::Msg::FocusQuery),
        KeyCode::Esc => Some(search::Msg::Blur),
        _ => None,
    }
}

fn decode_digest(key: KeyEvent, confirm: bool) -> Option<digest::Msg> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if confirm {
        return match key.code {
            KeyCode::Char('y' | 'Y') | KeyCode::Enter => Some(digest::Msg::Confirm),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(digest::Msg::Cancel),
            _ => None,
        };
    }
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => Some(digest::Msg::Down),
        KeyCode::Char('k') | KeyCode::Up => Some(digest::Msg::Up),
        KeyCode::Char('d') if ctrl => Some(digest::Msg::ScrollDown),
        KeyCode::Char('u') if ctrl => Some(digest::Msg::ScrollUp),
        KeyCode::PageDown => Some(digest::Msg::ScrollDown),
        KeyCode::PageUp => Some(digest::Msg::ScrollUp),
        KeyCode::Char('f') => Some(digest::Msg::CycleFilter),
        KeyCode::Char('g') => Some(digest::Msg::RequestGenerate),
        KeyCode::Char('r') => Some(digest::Msg::Refresh),
        _ => None,
    }
}

// --- Review detail loading + pre-flight (unchanged behavior) ---

/// Load one proposal's patch, parse it, and compute the pre-flight verdict.
fn load_detail(vault: &Vault, proposals_dir: &str, id: &str) -> Result<Loaded, String> {
    let (proposal, patch) =
        curator_core::load_proposal(vault, proposals_dir, id).map_err(|e| e.to_string())?;
    let (file_patches, parse_error) = match curator_librarian::parse_patch(&patch) {
        Ok(fps) => (fps, None),
        Err(e) => (Vec::new(), Some(e.to_string())),
    };
    let preflight = preflight(vault, &proposal, &file_patches, parse_error.as_deref());
    Ok(Loaded {
        proposal,
        file_patches,
        parse_error,
        preflight,
    })
}

/// Would this proposal's patch apply cleanly against the CURRENT vault?
///
/// Uses only the pure, non-destructive `apply_file_patch` — it never calls
/// `apply_proposal` (which permanently stamps `rejected` on failure). It
/// catches the common irreversible-reject causes — an unparseable or empty
/// changeset, drift on a modify, and create-over-existing — so the reviewer
/// is warned before they commit. It does NOT reproduce every validator rule
/// (Curio managed-region / identity-collision rejects still surface only at
/// real apply time).
fn preflight(
    vault: &Vault,
    proposal: &Proposal,
    file_patches: &[FilePatch],
    parse_error: Option<&str>,
) -> Preflight {
    if proposal.status != ProposalStatus::Open {
        return Preflight::default(); // terminal states never apply
    }
    // A patch that does not parse — or parses to nothing — is a guaranteed
    // apply-time hard-reject. Never show it as clean: that is exactly the
    // false-green this pre-flight exists to prevent (and `file_patches` is
    // empty in both cases, so the per-file loop below would otherwise find
    // no trouble and report "applies cleanly").
    if parse_error.is_some() {
        return Preflight {
            applies_clean: false,
            warning: Some("patch does not parse — apply would reject it".to_owned()),
        };
    }
    if file_patches.is_empty() {
        return Preflight {
            applies_clean: false,
            warning: Some("empty changeset — apply would reject it".to_owned()),
        };
    }
    let mut trouble: Vec<String> = Vec::new();
    for fp in file_patches {
        match &fp.old_path {
            // Creation: the target must not already exist, and must build
            // from an empty base.
            None => {
                if vault.read(&fp.new_path).is_ok() {
                    trouble.push(format!("{} already exists", fp.new_path));
                } else if curator_librarian::apply_file_patch("", fp).is_err() {
                    trouble.push(fp.new_path.clone());
                }
            }
            // Modification: the current content must accept the patch with
            // zero fuzz.
            Some(path) => match vault.read(path) {
                Ok(current) => {
                    if curator_librarian::apply_file_patch(&current, fp).is_err() {
                        trouble.push(fp.new_path.clone());
                    }
                }
                Err(_) => trouble.push(format!("{path} is missing")),
            },
        }
    }
    if trouble.is_empty() {
        Preflight {
            applies_clean: true,
            warning: None,
        }
    } else {
        Preflight {
            applies_clean: false,
            warning: Some(format!(
                "vault drift — may not apply cleanly: {}",
                trouble.join("; ")
            )),
        }
    }
}

fn apply_error_flash(e: &ApplyError) -> Flash {
    match e {
        ApplyError::Rejected { reason, .. } => Flash::error(format!(
            "rejected: {reason} (terminal — fix and re-propose)"
        )),
        ApplyError::NotOpen { status, .. } => Flash::warn(format!("already {status}")),
        ApplyError::Store(_) | ApplyError::Vault(_) => {
            Flash::warn(format!("environment error (retryable): {e}"))
        }
    }
}

fn reject_error_flash(e: &RejectError) -> Flash {
    match e {
        RejectError::NotOpen { status, .. } => Flash::warn(format!("already {status}")),
        RejectError::Store(_) => Flash::warn(format!("environment error (retryable): {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use curator_librarian::{Hunk, HunkLine};

    fn open(id: &str) -> Proposal {
        Proposal {
            schema: "proposals/v1".to_owned(),
            id: id.to_owned(),
            created: "2026-07-07T00:00:00Z".to_owned(),
            author: "test".to_owned(),
            title: "a change".to_owned(),
            rationale: "why".to_owned(),
            status: ProposalStatus::Open,
            files: vec!["notes/x.md".to_owned()],
        }
    }

    fn empty_vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = Vault::open(dir.path()).expect("open vault");
        (dir, vault)
    }

    // The bug the review caught: an unparseable patch leaves `file_patches`
    // empty, so the per-file loop finds no trouble — but a real apply hard-
    // rejects it. Pre-flight must NOT report that as clean.
    #[test]
    fn unparseable_patch_is_flagged_not_clean() {
        let (_dir, vault) = empty_vault();
        let pf = preflight(&vault, &open("01AAA"), &[], Some("malformed hunk header"));
        assert!(!pf.applies_clean);
        assert!(
            pf.warning
                .as_deref()
                .unwrap_or_default()
                .contains("does not parse"),
            "got {:?}",
            pf.warning
        );
    }

    // An empty changeset parses to zero patches (parse_error None) and also
    // hard-rejects at apply — likewise never "clean".
    #[test]
    fn empty_changeset_is_flagged_not_clean() {
        let (_dir, vault) = empty_vault();
        let pf = preflight(&vault, &open("01BBB"), &[], None);
        assert!(!pf.applies_clean);
        assert!(
            pf.warning.as_deref().unwrap_or_default().contains("empty"),
            "got {:?}",
            pf.warning
        );
    }

    // A single clean creation against an empty vault is genuinely clean.
    #[test]
    fn clean_creation_reports_clean() {
        let (_dir, vault) = empty_vault();
        let fp = FilePatch {
            old_path: None,
            new_path: "notes/x.md".to_owned(),
            hunks: vec![Hunk {
                old_start: 0,
                old_len: 0,
                new_start: 1,
                new_len: 1,
                lines: vec![HunkLine::Add("# X".to_owned(), true)],
            }],
        };
        let pf = preflight(&vault, &open("01CCC"), std::slice::from_ref(&fp), None);
        assert!(pf.applies_clean, "warning: {:?}", pf.warning);
        assert!(pf.warning.is_none());
    }

    // Create-over-existing is a known reject cause — pre-flight catches it.
    #[test]
    fn creation_over_an_existing_file_is_flagged() {
        let (_dir, vault) = empty_vault();
        vault
            .write_atomic("notes/x.md", "already here\n")
            .expect("seed");
        let fp = FilePatch {
            old_path: None,
            new_path: "notes/x.md".to_owned(),
            hunks: vec![Hunk {
                old_start: 0,
                old_len: 0,
                new_start: 1,
                new_len: 1,
                lines: vec![HunkLine::Add("# X".to_owned(), true)],
            }],
        };
        let pf = preflight(&vault, &open("01DDD"), std::slice::from_ref(&fp), None);
        assert!(!pf.applies_clean);
        assert!(
            pf.warning
                .as_deref()
                .unwrap_or_default()
                .contains("already exists"),
            "got {:?}",
            pf.warning
        );
    }

    // Terminal proposals never apply — pre-flight is the (no-warning) default.
    #[test]
    fn terminal_proposal_yields_the_default() {
        let (_dir, vault) = empty_vault();
        let mut p = open("01EEE");
        p.status = ProposalStatus::Applied;
        let pf = preflight(&vault, &p, &[], None);
        assert!(!pf.applies_clean);
        assert!(pf.warning.is_none());
    }
}
