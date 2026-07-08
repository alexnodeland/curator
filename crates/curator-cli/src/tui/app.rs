//! Pure state + reducer for the `curator review` TUI.
//!
//! This layer holds NO terminal. It is a plain state machine: feed it
//! [`Msg`]s (already decoded from key events) and it returns the next
//! [`Action`] for the event loop to execute (apply / reject / reload /
//! quit). That split keeps all behaviour unit-testable without a TTY — the
//! render + event pump in [`super`] is a dumb adapter over this module.

use curator_core::{Proposal, ProposalStatus};
use curator_librarian::FilePatch;

use super::common::Flash;

/// The first N chars of a ULID — enough to identify a proposal on screen.
#[must_use]
pub fn short_id(id: &str) -> String {
    id.chars().take(10).collect()
}

/// The human label for a status (matches the on-disk `lowercase` form).
#[must_use]
pub fn status_label(status: ProposalStatus) -> &'static str {
    match status {
        ProposalStatus::Open => "open",
        ProposalStatus::Applied => "applied",
        ProposalStatus::Rejected => "rejected",
    }
}

/// Which proposals the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusFilter {
    All,
    Open,
    Applied,
    Rejected,
}

impl StatusFilter {
    fn matches(self, status: ProposalStatus) -> bool {
        match self {
            StatusFilter::All => true,
            StatusFilter::Open => status == ProposalStatus::Open,
            StatusFilter::Applied => status == ProposalStatus::Applied,
            StatusFilter::Rejected => status == ProposalStatus::Rejected,
        }
    }

    fn next(self) -> Self {
        match self {
            StatusFilter::All => StatusFilter::Open,
            StatusFilter::Open => StatusFilter::Applied,
            StatusFilter::Applied => StatusFilter::Rejected,
            StatusFilter::Rejected => StatusFilter::All,
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            StatusFilter::All => "all",
            StatusFilter::Open => "open",
            StatusFilter::Applied => "applied",
            StatusFilter::Rejected => "rejected",
        }
    }
}

/// The pre-flight verdict for the selected proposal: does its patch still
/// apply cleanly against the CURRENT vault? Computed with the public
/// `parse_patch` + `apply_file_patch` (never `apply_proposal`, which is
/// destructive-on-reject), so the reviewer sees drift BEFORE they commit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preflight {
    pub applies_clean: bool,
    /// A human note when it may NOT apply (drift / create-over-existing).
    pub warning: Option<String>,
}

/// The detail loaded for the selected proposal: its parsed patch + the
/// pre-flight verdict. Built by the event loop (it needs vault I/O), held
/// here as inert data the view renders.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub proposal: Proposal,
    pub file_patches: Vec<FilePatch>,
    /// Set when `changes.patch` could not be parsed (shows the raw error).
    pub parse_error: Option<String>,
    pub preflight: Preflight,
}

/// A pending confirmation the reviewer must accept before it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    Apply(String),
    Reject(String),
}

/// Which overlay, if any, is active over the browse view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Browse,
    Confirm(Pending),
}

/// A decoded intent from a key press — the reducer's input alphabet.
/// Quit and help are the shell's concern (global), never the screen's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    Up,
    Down,
    ScrollUp,
    ScrollDown,
    CycleFilter,
    RequestApply,
    RequestReject,
    Confirm,
    Cancel,
    Refresh,
}

/// A side-effecting instruction the event loop executes after `update`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    /// Re-list proposals from disk (status changed / manual refresh).
    Reload,
    Apply(String),
    Reject(String),
}

/// Diff scroll step per key press (lines). Page-ish, since the diff is the
/// bulk of the detail pane and per-line scroll would be tedious.
const SCROLL_STEP: u16 = 10;

/// The whole reviewer state.
#[derive(Debug)]
pub struct ReviewApp {
    proposals: Vec<Proposal>,
    filter: StatusFilter,
    /// Selection INTO the filtered (visible) list, not `proposals`.
    selected: usize,
    diff_scroll: u16,
    mode: Mode,
    flash: Option<Flash>,
}

impl ReviewApp {
    #[must_use]
    pub fn new(proposals: Vec<Proposal>) -> Self {
        Self {
            proposals,
            filter: StatusFilter::All,
            selected: 0,
            diff_scroll: 0,
            mode: Mode::Browse,
            flash: None,
        }
    }

    // --- accessors the view / loop read ---

    #[must_use]
    pub fn filter(&self) -> StatusFilter {
        self.filter
    }
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }
    #[must_use]
    pub fn diff_scroll(&self) -> u16 {
        self.diff_scroll
    }
    #[must_use]
    pub fn mode(&self) -> &Mode {
        &self.mode
    }
    #[must_use]
    pub fn flash(&self) -> Option<&Flash> {
        self.flash.as_ref()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.proposals.is_empty()
    }

    /// The proposals matching the active filter, in list (ULID) order.
    #[must_use]
    pub fn visible(&self) -> Vec<&Proposal> {
        self.proposals
            .iter()
            .filter(|p| self.filter.matches(p.status))
            .collect()
    }

    /// The currently-highlighted proposal, if any is visible.
    #[must_use]
    pub fn selected_proposal(&self) -> Option<&Proposal> {
        self.visible().into_iter().nth(self.selected)
    }

    /// The id of the highlighted proposal — the event loop keys detail
    /// loading off this.
    #[must_use]
    pub fn selected_id(&self) -> Option<&str> {
        self.selected_proposal().map(|p| p.id.as_str())
    }

    fn visible_len(&self) -> usize {
        self.proposals
            .iter()
            .filter(|p| self.filter.matches(p.status))
            .count()
    }

    // --- the reducer ---

    /// Advance the state machine; returns the [`Action`] the loop must run.
    pub fn update(&mut self, msg: Msg) -> Action {
        match self.mode.clone() {
            Mode::Confirm(pending) => self.update_confirm(msg, pending),
            Mode::Browse => self.update_browse(msg),
        }
    }

    /// The reviewer is showing a confirm overlay that should capture keys —
    /// the shell forwards everything to it rather than treating keys as
    /// global shortcuts.
    #[must_use]
    pub fn is_modal(&self) -> bool {
        matches!(self.mode, Mode::Confirm(_))
    }

    fn update_confirm(&mut self, msg: Msg, pending: Pending) -> Action {
        match msg {
            Msg::Confirm => {
                self.mode = Mode::Browse;
                match pending {
                    Pending::Apply(id) => Action::Apply(id),
                    Pending::Reject(id) => Action::Reject(id),
                }
            }
            Msg::Cancel => {
                self.mode = Mode::Browse;
                Action::None
            }
            _ => Action::None,
        }
    }

    fn update_browse(&mut self, msg: Msg) -> Action {
        match msg {
            Msg::Refresh => return Action::Reload,
            Msg::RequestApply => return self.request(true),
            Msg::RequestReject => return self.request(false),
            Msg::Down => self.move_selection(1),
            Msg::Up => self.move_selection(-1),
            Msg::ScrollDown => self.diff_scroll = self.diff_scroll.saturating_add(SCROLL_STEP),
            Msg::ScrollUp => self.diff_scroll = self.diff_scroll.saturating_sub(SCROLL_STEP),
            Msg::CycleFilter => {
                self.filter = self.filter.next();
                self.clamp_selection();
                self.diff_scroll = 0;
                self.flash = None;
            }
            // Confirm / Cancel are meaningless outside an overlay.
            Msg::Confirm | Msg::Cancel => {}
        }
        Action::None
    }

    /// Open a confirm overlay for apply (`true`) / reject (`false`) — but
    /// only for an OPEN proposal; the terminal states are guarded up front,
    /// matching the library's one-way transition rule.
    fn request(&mut self, apply: bool) -> Action {
        let Some((id, status)) = self.selected_proposal().map(|p| (p.id.clone(), p.status)) else {
            self.flash = Some(Flash::warn("no proposal selected"));
            return Action::None;
        };
        if status != ProposalStatus::Open {
            self.flash = Some(Flash::warn(format!(
                "proposal is {} — only open proposals can be {}",
                status_label(status),
                if apply { "applied" } else { "rejected" }
            )));
            return Action::None;
        }
        self.mode = Mode::Confirm(if apply {
            Pending::Apply(id)
        } else {
            Pending::Reject(id)
        });
        self.flash = None;
        Action::None
    }

    fn move_selection(&mut self, delta: i32) {
        let len = self.visible_len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        let max = (len - 1) as i32;
        let next = (self.selected as i32 + delta).clamp(0, max) as usize;
        if next != self.selected {
            self.selected = next;
            self.diff_scroll = 0;
            self.flash = None;
        }
    }

    fn clamp_selection(&mut self) {
        let len = self.visible_len();
        self.selected = if len == 0 {
            0
        } else {
            self.selected.min(len - 1)
        };
    }

    // --- mutations the event loop drives ---

    /// Replace the proposal list (after an apply / reject / refresh),
    /// keeping the selection index in range.
    pub fn reload(&mut self, proposals: Vec<Proposal>) {
        self.proposals = proposals;
        self.clamp_selection();
    }

    /// Select the proposal with `id`, if it is visible under the current
    /// filter. Used to jump to a freshly-created proposal (a just-generated
    /// digest sorts last by ULID, so the caller must seek it, not assume 0).
    pub fn select_by_id(&mut self, id: &str) {
        if let Some(idx) = self.visible().iter().position(|p| p.id == id) {
            self.selected = idx;
            self.diff_scroll = 0;
        }
    }

    /// Dismiss a pending confirm overlay — e.g. when the user tabs away — so a
    /// stale "apply?/reject?" can't later be confirmed by a reflexive
    /// keypress on a screen the reviewer thinks is idle.
    pub fn cancel_confirm(&mut self) {
        if matches!(self.mode, Mode::Confirm(_)) {
            self.mode = Mode::Browse;
        }
    }

    pub fn set_flash(&mut self, flash: Flash) {
        self.flash = Some(flash);
    }
}

#[cfg(test)]
mod tests {
    use super::super::common::FlashLevel;
    use super::*;

    fn prop(id: &str, status: ProposalStatus) -> Proposal {
        Proposal {
            schema: "proposals/v1".to_owned(),
            id: id.to_owned(),
            created: "2026-07-07T00:00:00Z".to_owned(),
            author: "test".to_owned(),
            title: format!("proposal {id}"),
            rationale: "why".to_owned(),
            status,
            files: vec!["notes/x.md".to_owned()],
        }
    }

    fn three() -> ReviewApp {
        ReviewApp::new(vec![
            prop("01AAA", ProposalStatus::Open),
            prop("01BBB", ProposalStatus::Applied),
            prop("01CCC", ProposalStatus::Rejected),
        ])
    }

    #[test]
    fn navigation_clamps_at_both_ends() {
        let mut app = three();
        assert_eq!(app.selected(), 0);
        assert_eq!(app.update(Msg::Up), Action::None);
        assert_eq!(app.selected(), 0, "up at the top is a no-op");
        app.update(Msg::Down);
        app.update(Msg::Down);
        assert_eq!(app.selected(), 2);
        app.update(Msg::Down);
        assert_eq!(app.selected(), 2, "down at the bottom is a no-op");
        assert_eq!(app.selected_id(), Some("01CCC"));
    }

    #[test]
    fn filter_cycles_and_reclamps_selection() {
        let mut app = three();
        app.update(Msg::Down);
        app.update(Msg::Down); // select the 3rd (rejected) in `all`
        assert_eq!(app.selected(), 2);
        // all -> open: only one visible, selection clamps to 0.
        app.update(Msg::CycleFilter);
        assert_eq!(app.filter(), StatusFilter::Open);
        assert_eq!(app.selected(), 0);
        assert_eq!(app.selected_id(), Some("01AAA"));
        // open -> applied -> rejected -> all
        app.update(Msg::CycleFilter);
        assert_eq!(app.selected_id(), Some("01BBB"));
        app.update(Msg::CycleFilter);
        assert_eq!(app.selected_id(), Some("01CCC"));
        app.update(Msg::CycleFilter);
        assert_eq!(app.filter(), StatusFilter::All);
    }

    #[test]
    fn apply_request_on_open_confirms_then_returns_the_action() {
        let mut app = three();
        assert_eq!(app.update(Msg::RequestApply), Action::None);
        assert_eq!(
            app.mode(),
            &Mode::Confirm(Pending::Apply("01AAA".to_owned()))
        );
        // Confirm fires the Action and returns to Browse.
        assert_eq!(app.update(Msg::Confirm), Action::Apply("01AAA".to_owned()));
        assert_eq!(app.mode(), &Mode::Browse);
    }

    #[test]
    fn reject_request_then_cancel_returns_to_browse() {
        let mut app = three();
        app.update(Msg::RequestReject);
        assert_eq!(
            app.mode(),
            &Mode::Confirm(Pending::Reject("01AAA".to_owned()))
        );
        assert_eq!(app.update(Msg::Cancel), Action::None);
        assert_eq!(app.mode(), &Mode::Browse);
    }

    #[test]
    fn apply_request_on_non_open_is_refused_with_a_flash() {
        let mut app = three();
        app.update(Msg::Down); // 01BBB, applied
        assert_eq!(app.update(Msg::RequestApply), Action::None);
        assert_eq!(app.mode(), &Mode::Browse, "no confirm overlay opens");
        let flash = app.flash().expect("a warning flash");
        assert_eq!(flash.level, FlashLevel::Warn);
        assert!(flash.text.contains("applied"), "got {:?}", flash.text);
    }

    #[test]
    fn confirm_overlay_is_modal_so_the_shell_yields_all_keys() {
        let mut app = three();
        assert!(!app.is_modal(), "browse is not modal");
        app.update(Msg::RequestApply);
        assert!(app.is_modal(), "a confirm overlay captures keys");
        app.update(Msg::Cancel);
        assert!(!app.is_modal(), "cancel returns to browse");
    }

    #[test]
    fn refresh_asks_the_loop_to_reload() {
        let mut app = three();
        assert_eq!(app.update(Msg::Refresh), Action::Reload);
    }

    #[test]
    fn select_by_id_jumps_to_a_freshly_filed_proposal() {
        // A generated digest sorts last by ULID; selection must seek it, not
        // assume index 0 (the regression the adversarial review caught).
        let mut app = three();
        assert_eq!(app.selected(), 0);
        app.select_by_id("01CCC");
        assert_eq!(app.selected(), 2);
        assert_eq!(app.selected_id(), Some("01CCC"));
        // An id not visible under the current filter is a no-op, not a panic.
        app.select_by_id("nope");
        assert_eq!(app.selected(), 2);
    }

    #[test]
    fn cancel_confirm_dismisses_a_pending_overlay() {
        let mut app = three();
        app.update(Msg::RequestApply);
        assert!(app.is_modal());
        app.cancel_confirm();
        assert!(!app.is_modal(), "tabbing away cancels the confirm");
        // Idempotent when there is nothing to cancel.
        app.cancel_confirm();
        assert!(!app.is_modal());
    }

    #[test]
    fn empty_queue_has_no_selection() {
        let mut app = ReviewApp::new(vec![]);
        assert!(app.is_empty());
        assert_eq!(app.selected_id(), None);
        assert_eq!(app.update(Msg::RequestApply), Action::None);
        assert!(app.flash().is_some(), "warns there is nothing selected");
    }
}
