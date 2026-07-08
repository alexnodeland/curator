//! `curator review` — the interactive proposal reviewer.
//!
//! A thin adapter around the pure [`app`] reducer: init the terminal, load
//! the proposal queue, then loop { draw; read a key → [`app::Msg`];
//! `app.update` → run the returned [`app::Action`] against the librarian }.
//! All behaviour lives in [`app`]; this file only touches the terminal and
//! the vault/librarian, so it stays effectively logic-free (and untested).

mod app;
mod diff;
mod view;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use curator_core::{Proposal, ProposalStatus, Vault};
use curator_librarian::{ApplyError, FilePatch, RejectError};

use app::{Action, Flash, Loaded, Mode, Msg, Preflight, ReviewApp, short_id};

/// Launch the reviewer over the vault's proposal queue. Returns when the
/// user quits (or on a terminal I/O error). The terminal is always
/// restored — `ratatui::init` also installs a panic hook that restores it,
/// so a panic mid-draw won't wedge the user's shell.
pub fn run_review(vault: &Vault, proposals_dir: &str) -> Result<(), String> {
    let proposals =
        curator_core::list_proposals(vault, proposals_dir).map_err(|e| e.to_string())?;
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, vault, proposals_dir, proposals);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    vault: &Vault,
    proposals_dir: &str,
    proposals: Vec<Proposal>,
) -> Result<(), String> {
    let mut app = ReviewApp::new(proposals);
    let mut loaded: Option<Loaded> = None;
    let mut loaded_id: Option<String> = None;

    loop {
        // Refresh the detail pane whenever the selection changes.
        let sel = app.selected_id().map(str::to_owned);
        if sel != loaded_id {
            loaded = match sel.as_deref() {
                Some(id) => match load_detail(vault, proposals_dir, id) {
                    Ok(l) => Some(l),
                    Err(e) => {
                        app.set_flash(Flash::warn(e));
                        None
                    }
                },
                None => None,
            };
            loaded_id = sel;
        }

        terminal
            .draw(|f| view::render(f, &app, loaded.as_ref()))
            .map_err(|e| e.to_string())?;

        let Event::Key(key) = event::read().map_err(|e| e.to_string())? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue; // ignore key-release / repeat on platforms that send them
        }
        let Some(msg) = decode_key(key, app.mode()) else {
            continue;
        };

        match app.update(msg) {
            Action::None => {}
            Action::Quit => break,
            Action::Reload => {
                reload(&mut app, vault, proposals_dir)?;
                loaded_id = None;
            }
            Action::Apply(id) => {
                let flash = match curator_librarian::apply_proposal(vault, proposals_dir, &id) {
                    Ok(report) => Flash::success(format!(
                        "applied {} — {} file(s) written",
                        short_id(&report.id),
                        report.files_written.len()
                    )),
                    Err(e) => apply_error_flash(&e),
                };
                app.set_flash(flash);
                reload(&mut app, vault, proposals_dir)?;
                loaded_id = None;
            }
            Action::Reject(id) => {
                let flash = match curator_librarian::reject_proposal(vault, proposals_dir, &id) {
                    Ok(p) => Flash::success(format!("rejected {} — {}", short_id(&p.id), p.title)),
                    Err(e) => reject_error_flash(&e),
                };
                app.set_flash(flash);
                reload(&mut app, vault, proposals_dir)?;
                loaded_id = None;
            }
        }
    }
    Ok(())
}

fn reload(app: &mut ReviewApp, vault: &Vault, proposals_dir: &str) -> Result<(), String> {
    let proposals =
        curator_core::list_proposals(vault, proposals_dir).map_err(|e| e.to_string())?;
    app.reload(proposals);
    Ok(())
}

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

/// Map a key press to a [`Msg`], mode-dependent. Returns `None` for keys
/// the current mode ignores.
fn decode_key(key: KeyEvent, mode: &Mode) -> Option<Msg> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match mode {
        Mode::Confirm(_) => match key.code {
            KeyCode::Char('y' | 'Y') | KeyCode::Enter => Some(Msg::Confirm),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => Some(Msg::Cancel),
            _ => None,
        },
        Mode::Help => match key.code {
            KeyCode::Char('q') => Some(Msg::Quit),
            _ => Some(Msg::ToggleHelp), // any other key dismisses
        },
        Mode::Browse => match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Some(Msg::Quit),
            KeyCode::Char('j') | KeyCode::Down => Some(Msg::Down),
            KeyCode::Char('k') | KeyCode::Up => Some(Msg::Up),
            KeyCode::Char('d') if ctrl => Some(Msg::ScrollDown),
            KeyCode::Char('u') if ctrl => Some(Msg::ScrollUp),
            KeyCode::PageDown => Some(Msg::ScrollDown),
            KeyCode::PageUp => Some(Msg::ScrollUp),
            KeyCode::Char('f') => Some(Msg::CycleFilter),
            KeyCode::Char('a') => Some(Msg::RequestApply),
            KeyCode::Char('x') => Some(Msg::RequestReject),
            KeyCode::Char('r') => Some(Msg::Refresh),
            KeyCode::Char('?') => Some(Msg::ToggleHelp),
            _ => None,
        },
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
