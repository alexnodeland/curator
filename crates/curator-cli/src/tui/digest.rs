//! The Digest screen: a read-only preview of what the deterministic
//! librarian would surface right now — ranked candidates, scores, and the
//! why-surfaced reasons — with a one-key **generate** that files the digest
//! as a `proposals/v1` proposal (which you then review on the Review tab).
//!
//! The reducer holds NO terminal and does NO I/O. The event loop runs
//! `curator_librarian::preview_digest` / `run_digest` and feeds the outcome
//! back through [`DigestApp::set_preview`]; the `curator_librarian` types are
//! mapped to the owned [`Row`] here, keeping the reducer library-free and
//! unit-testable.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use super::common::{centered_rect, kv};

/// Key hints shown in the shell footer while the Digest screen is active.
pub const HINT: &str = " j/k move · ^d/^u scroll · f filter · g generate · r refresh";

/// One ranked candidate row — the owned projection of a `RankedCandidate` +
/// its `Candidate`.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub title: String,
    pub path: String,
    pub tags: Vec<String>,
    pub source: Option<String>,
    pub score: f64,
    pub similarity: Option<f64>,
    pub age_days: f64,
    pub why: String,
    /// Lands in the surfaced top-k (`true`) or the quiet tail (`false`).
    pub surfaced: bool,
    /// A one-line extractive preview of the note body.
    pub preview: String,
}

/// The mapped, inert preview the view renders.
#[derive(Debug, Clone)]
pub struct Preview {
    pub date: String,
    pub note_path: String,
    pub rows: Vec<Row>,
    pub surfaced: usize,
    pub quiet: usize,
    pub warnings: Vec<String>,
    /// A digest for `date` already exists — generating would be a no-op.
    pub already_exists: bool,
}

/// Which candidates the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Surfaced,
    Quiet,
}

impl Filter {
    fn matches(self, surfaced: bool) -> bool {
        match self {
            Filter::All => true,
            Filter::Surfaced => surfaced,
            Filter::Quiet => !surfaced,
        }
    }
    fn next(self) -> Self {
        match self {
            Filter::All => Filter::Surfaced,
            Filter::Surfaced => Filter::Quiet,
            Filter::Quiet => Filter::All,
        }
    }
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Surfaced => "surfaced",
            Filter::Quiet => "quiet",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    Browse,
    /// Confirm generating today's digest.
    Confirm,
}

use super::common::Flash;

/// A decoded intent — the reducer's input alphabet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    Up,
    Down,
    ScrollUp,
    ScrollDown,
    CycleFilter,
    RequestGenerate,
    Confirm,
    Cancel,
    Refresh,
}

/// A side-effecting instruction the event loop runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    /// Re-run `preview_digest` from the index.
    Reload,
    /// Generate today's digest (files a proposal).
    Generate,
}

const SCROLL_STEP: u16 = 10;

/// The Digest screen state.
#[derive(Debug)]
pub struct DigestApp {
    preview: Option<Preview>,
    filter: Filter,
    selected: usize,
    body_scroll: u16,
    mode: Mode,
    flash: Option<Flash>,
}

impl Default for DigestApp {
    fn default() -> Self {
        Self {
            preview: None,
            filter: Filter::All,
            selected: 0,
            body_scroll: 0,
            mode: Mode::Browse,
            flash: None,
        }
    }
}

impl DigestApp {
    // --- accessors ---

    #[must_use]
    pub fn preview(&self) -> Option<&Preview> {
        self.preview.as_ref()
    }
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.preview.is_some()
    }
    #[must_use]
    pub fn filter(&self) -> Filter {
        self.filter
    }
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }
    #[must_use]
    pub fn body_scroll(&self) -> u16 {
        self.body_scroll
    }
    #[must_use]
    pub fn confirming(&self) -> bool {
        self.mode == Mode::Confirm
    }
    #[must_use]
    pub fn is_modal(&self) -> bool {
        self.mode == Mode::Confirm
    }
    #[must_use]
    pub fn flash(&self) -> Option<&Flash> {
        self.flash.as_ref()
    }

    /// The rows matching the active filter, in rank order.
    #[must_use]
    pub fn visible(&self) -> Vec<&Row> {
        match &self.preview {
            None => Vec::new(),
            Some(p) => p
                .rows
                .iter()
                .filter(|r| self.filter.matches(r.surfaced))
                .collect(),
        }
    }

    #[must_use]
    pub fn selected_row(&self) -> Option<&Row> {
        self.visible().into_iter().nth(self.selected)
    }

    fn visible_len(&self) -> usize {
        self.visible().len()
    }

    // --- the reducer ---

    pub fn update(&mut self, msg: Msg) -> Action {
        if self.mode == Mode::Confirm {
            return self.update_confirm(msg);
        }
        match msg {
            Msg::Refresh => return Action::Reload,
            Msg::RequestGenerate => return self.request_generate(),
            Msg::Down => self.move_selection(1),
            Msg::Up => self.move_selection(-1),
            Msg::ScrollDown => self.body_scroll = self.body_scroll.saturating_add(SCROLL_STEP),
            Msg::ScrollUp => self.body_scroll = self.body_scroll.saturating_sub(SCROLL_STEP),
            Msg::CycleFilter => {
                self.filter = self.filter.next();
                self.clamp_selection();
                self.body_scroll = 0;
                self.flash = None;
            }
            Msg::Confirm | Msg::Cancel => {}
        }
        Action::None
    }

    fn update_confirm(&mut self, msg: Msg) -> Action {
        match msg {
            Msg::Confirm => {
                self.mode = Mode::Browse;
                Action::Generate
            }
            Msg::Cancel => {
                self.mode = Mode::Browse;
                Action::None
            }
            _ => Action::None,
        }
    }

    fn request_generate(&mut self) -> Action {
        match &self.preview {
            None => {
                self.flash = Some(Flash::warn("still loading — nothing to generate yet"));
                Action::None
            }
            Some(p) if p.already_exists => {
                self.flash = Some(Flash::warn(format!(
                    "today's digest already exists ({}) — generating is a no-op",
                    p.note_path
                )));
                Action::None
            }
            Some(p) if p.rows.is_empty() => {
                self.flash = Some(Flash::warn("nothing new to surface — nothing to generate"));
                Action::None
            }
            Some(_) => {
                self.mode = Mode::Confirm;
                self.flash = None;
                Action::None
            }
        }
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
            self.body_scroll = 0;
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

    // --- setters the effect loop drives ---

    /// Install a fresh preview, keeping the selection in range.
    pub fn set_preview(&mut self, preview: Preview) {
        self.preview = Some(preview);
        self.clamp_selection();
    }

    /// Dismiss a pending generate-confirm overlay (e.g. on tab-away) so it
    /// can't be confirmed later by a reflexive keypress.
    pub fn cancel_confirm(&mut self) {
        if self.mode == Mode::Confirm {
            self.mode = Mode::Browse;
        }
    }

    pub fn set_flash(&mut self, flash: Flash) {
        self.flash = Some(flash);
    }
}

// --- view ---

/// Draw the Digest screen into `area`.
pub fn render(frame: &mut Frame, app: &DigestApp, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);
    render_header(frame, app, rows[0]);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(44), Constraint::Min(20)])
        .split(rows[1]);
    render_list(frame, app, cols[0]);
    render_detail(frame, app, cols[1]);

    if app.confirming() {
        render_confirm(frame, app, area);
    }
}

fn render_header(frame: &mut Frame, app: &DigestApp, area: Rect) {
    let line = match app.preview() {
        None => Line::from(Span::styled(
            " loading digest preview…",
            Style::default().fg(Color::DarkGray),
        )),
        Some(p) => {
            let mut spans = vec![
                Span::styled(
                    format!(" digest {} ", p.date),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("· {} surfaced · {} quiet", p.surfaced, p.quiet),
                    Style::default().fg(Color::DarkGray),
                ),
            ];
            if p.already_exists {
                spans.push(Span::styled(
                    "  · already generated",
                    Style::default().fg(Color::Yellow),
                ));
            }
            if let Some(warning) = p.warnings.first() {
                spans.push(Span::styled(
                    format!("  ⚠ {warning}"),
                    Style::default().fg(Color::Yellow),
                ));
            }
            Line::from(spans)
        }
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn render_list(frame: &mut Frame, app: &DigestApp, area: Rect) {
    let filter = app.filter();
    let visible = app.visible();
    let title = format!(" candidates · {} ({}) ", filter.label(), visible.len());

    if app.is_loaded() && visible.is_empty() {
        let msg = if app.preview().map(|p| p.rows.is_empty()).unwrap_or(true) {
            "Nothing new to surface since the last digest.".to_owned()
        } else {
            format!("No candidates match the '{}' filter.", filter.label())
        };
        frame.render_widget(
            Paragraph::new(msg)
                .block(Block::default().borders(Borders::ALL).title(title))
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = visible
        .iter()
        .map(|r| {
            let (glyph, color) = if r.surfaced {
                ("★", Color::Green)
            } else {
                ("·", Color::DarkGray)
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{glyph} "), Style::default().fg(color)),
                Span::styled(
                    format!("{:>6.3} ", r.score),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(r.title.clone()),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▍");
    let mut state = ListState::default();
    if !visible.is_empty() {
        state.select(Some(app.selected()));
    }
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_detail(frame: &mut Frame, app: &DigestApp, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" why surfaced ");
    if !app.is_loaded() {
        frame.render_widget(Paragraph::new("loading…").block(block), area);
        return;
    }
    let Some(r) = app.selected_row() else {
        frame.render_widget(
            Paragraph::new("Nothing selected.")
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    };

    let mut lines = vec![
        kv("title", &r.title),
        kv("path", &r.path),
        kv("score", &format!("{:.4}", r.score)),
    ];
    lines.push(kv(
        "similarity",
        &r.similarity
            .map(|s| format!("{s:.3}"))
            .unwrap_or_else(|| "— (recency-only)".to_owned()),
    ));
    lines.push(kv("age", &format!("{:.0}d", r.age_days)));
    if !r.tags.is_empty() {
        lines.push(kv("tags", &r.tags.join(", ")));
    }
    if let Some(src) = &r.source {
        lines.push(kv("source", src));
    }
    lines.push(Line::from(""));
    let banner = if r.surfaced {
        Span::styled(
            format!("★ surfaced — {}", r.why),
            Style::default().fg(Color::Green),
        )
    } else {
        Span::styled(
            format!("· quiet — {}", r.why),
            Style::default().fg(Color::DarkGray),
        )
    };
    lines.push(Line::from(banner));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "preview",
        Style::default().fg(Color::DarkGray),
    )));
    let preview = if r.preview.is_empty() {
        "(no preview)"
    } else {
        &r.preview
    };
    lines.push(Line::from(preview.to_owned()));

    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((app.body_scroll(), 0)),
        area,
    );
}

fn render_confirm(frame: &mut Frame, app: &DigestApp, area: Rect) {
    let note_path = app
        .preview()
        .map(|p| p.note_path.clone())
        .unwrap_or_default();
    let lines = vec![
        Line::from(Span::styled(
            "Generate today's digest?",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("creates {note_path}")),
        Line::from(Span::styled(
            "as a proposal you then review on the Review tab.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "[y]",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" generate    "),
            Span::styled(
                "[n]",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" cancel"),
        ]),
    ];
    let popup = centered_rect(60, 40, area);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" generate digest "),
            )
            .wrap(Wrap { trim: false }),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(title: &str, score: f64, surfaced: bool) -> Row {
        Row {
            title: title.to_owned(),
            path: format!("{title}.md"),
            tags: vec!["rust".to_owned()],
            source: None,
            score,
            similarity: Some(score),
            age_days: 3.0,
            why: "similarity 0.50 · 3d old".to_owned(),
            surfaced,
            preview: "a line".to_owned(),
        }
    }

    fn preview(already: bool, rows: Vec<Row>) -> Preview {
        let surfaced = rows.iter().filter(|r| r.surfaced).count();
        let quiet = rows.len() - surfaced;
        Preview {
            date: "2026-07-08".to_owned(),
            note_path: "digests/2026-07-08.md".to_owned(),
            rows,
            surfaced,
            quiet,
            warnings: vec![],
            already_exists: already,
        }
    }

    fn loaded() -> DigestApp {
        let mut app = DigestApp::default();
        app.set_preview(preview(
            false,
            vec![
                row("A", 0.9, true),
                row("B", 0.5, true),
                row("C", 0.1, false),
            ],
        ));
        app
    }

    #[test]
    fn filter_narrows_to_surfaced_and_quiet() {
        let mut app = loaded();
        assert_eq!(app.visible().len(), 3);
        app.update(Msg::CycleFilter); // surfaced
        assert_eq!(app.filter(), Filter::Surfaced);
        assert_eq!(app.visible().len(), 2);
        app.update(Msg::CycleFilter); // quiet
        assert_eq!(app.filter(), Filter::Quiet);
        assert_eq!(app.visible().len(), 1);
        assert_eq!(app.selected(), 0, "selection reclamps under the filter");
    }

    #[test]
    fn generate_asks_to_confirm_then_returns_the_action() {
        let mut app = loaded();
        assert_eq!(app.update(Msg::RequestGenerate), Action::None);
        assert!(app.confirming());
        assert_eq!(app.update(Msg::Confirm), Action::Generate);
        assert!(!app.confirming());
    }

    #[test]
    fn generate_is_refused_when_a_digest_already_exists() {
        let mut app = DigestApp::default();
        app.set_preview(preview(true, vec![row("A", 0.9, true)]));
        assert_eq!(app.update(Msg::RequestGenerate), Action::None);
        assert!(!app.confirming(), "no confirm opens");
        assert!(app.flash().unwrap().text.contains("already exists"));
    }

    #[test]
    fn generate_is_refused_with_no_candidates() {
        let mut app = DigestApp::default();
        app.set_preview(preview(false, vec![]));
        assert_eq!(app.update(Msg::RequestGenerate), Action::None);
        assert!(!app.confirming());
        assert!(app.flash().unwrap().text.contains("nothing"));
    }

    #[test]
    fn refresh_asks_the_loop_to_reload() {
        let mut app = loaded();
        assert_eq!(app.update(Msg::Refresh), Action::Reload);
    }

    #[test]
    fn cancel_confirm_dismisses_the_generate_overlay() {
        let mut app = loaded();
        app.update(Msg::RequestGenerate);
        assert!(app.confirming());
        app.cancel_confirm();
        assert!(!app.confirming(), "tabbing away drops the generate confirm");
    }

    #[test]
    fn navigation_clamps_within_the_filtered_rows() {
        let mut app = loaded();
        app.update(Msg::Up);
        assert_eq!(app.selected(), 0);
        app.update(Msg::Down);
        app.update(Msg::Down);
        app.update(Msg::Down);
        assert_eq!(app.selected(), 2, "clamped at the last of three");
    }
}
