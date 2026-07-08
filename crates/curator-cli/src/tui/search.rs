//! The Search screen: an interactive hybrid-search front end over the same
//! `KpEngine` the MCP surface and `curator search` ride.
//!
//! Like the reviewer, the reducer here holds NO terminal and does NO I/O:
//! [`SearchApp::update`] turns a [`Msg`] into an [`Action`] the event loop
//! runs against the engine, then feeds the outcome back through the setters
//! ([`SearchApp::set_results`], [`SearchApp::set_opened`], …). The engine's
//! `curator_mcp` types are mapped to the small owned structs below at that
//! boundary, so this module stays engine-free and unit-testable.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use super::common::{Flash, kv};

/// Key hints shown in the shell footer while the Search screen is active.
pub const HINT: &str = " type to search · ↵ run · j/k move · o open · r related · m mode · / edit";

/// One search hit — the owned projection of the engine's `HitOutput`.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub score: f64,
    pub id: String,
    pub title: String,
    pub path: String,
    pub snippet: String,
}

/// A fully-loaded note for the preview pane (engine `NoteOutput` projection).
#[derive(Debug, Clone, PartialEq)]
pub struct OpenedNote {
    pub id: String,
    pub title: String,
    pub path: String,
    pub tags: Vec<String>,
    pub source: Option<String>,
    pub ingested_at: String,
    pub content: String,
}

/// Retrieval mode — mirrors `curator search --mode`, mapped to the engine's
/// `SearchMode` at the effect boundary so the reducer stays engine-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Hybrid,
    Vector,
    Fts,
}

impl Mode {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Mode::Hybrid => "hybrid",
            Mode::Vector => "vector",
            Mode::Fts => "fts",
        }
    }
    fn next(self) -> Self {
        match self {
            Mode::Hybrid => Mode::Vector,
            Mode::Vector => Mode::Fts,
            Mode::Fts => Mode::Hybrid,
        }
    }
}

/// Which pane has the keyboard: the query box (typing) or the results list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Query,
    Results,
}

/// A decoded intent — the reducer's input alphabet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    Char(char),
    Backspace,
    Submit,
    FocusQuery,
    Blur,
    Up,
    Down,
    ScrollUp,
    ScrollDown,
    Open,
    Related,
    CycleMode,
}

/// A side-effecting instruction the event loop runs against the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    /// Run a query in the given mode; feed hits back via `set_results`.
    Search(String, Mode),
    /// Load one note's full content; feed back via `set_opened`.
    Open(String),
    /// Embedding-nearest to a note; feed hits back via `set_results`.
    Related(String),
}

const SCROLL_STEP: u16 = 10;

/// The Search screen state.
#[derive(Debug)]
pub struct SearchApp {
    query: String,
    mode: Mode,
    focus: Focus,
    results: Vec<Hit>,
    selected: usize,
    opened: Option<OpenedNote>,
    note_scroll: u16,
    /// A one-line result summary (`12 hits` / `no hits` / `related to …`).
    status: Option<String>,
    flash: Option<Flash>,
}

impl Default for SearchApp {
    fn default() -> Self {
        Self {
            query: String::new(),
            mode: Mode::Hybrid,
            focus: Focus::Query,
            results: Vec::new(),
            selected: 0,
            opened: None,
            note_scroll: 0,
            status: None,
            flash: None,
        }
    }
}

impl SearchApp {
    // --- accessors ---

    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }
    #[must_use]
    pub fn is_typing(&self) -> bool {
        self.focus == Focus::Query
    }
    #[must_use]
    pub fn results(&self) -> &[Hit] {
        &self.results
    }
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }
    #[must_use]
    pub fn selected_hit(&self) -> Option<&Hit> {
        self.results.get(self.selected)
    }
    #[must_use]
    pub fn opened(&self) -> Option<&OpenedNote> {
        self.opened.as_ref()
    }
    #[must_use]
    pub fn note_scroll(&self) -> u16 {
        self.note_scroll
    }
    #[must_use]
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }
    #[must_use]
    pub fn flash(&self) -> Option<&Flash> {
        self.flash.as_ref()
    }

    // --- the reducer ---

    /// Advance the state machine; returns the [`Action`] the loop must run.
    pub fn update(&mut self, msg: Msg) -> Action {
        self.flash = None;
        match self.focus {
            Focus::Query => self.update_query(msg),
            Focus::Results => self.update_results(msg),
        }
    }

    fn update_query(&mut self, msg: Msg) -> Action {
        match msg {
            Msg::Char(c) => {
                self.query.push(c);
                Action::None
            }
            Msg::Backspace => {
                self.query.pop();
                Action::None
            }
            Msg::Submit => self.run_search(),
            Msg::Blur => {
                self.focus = Focus::Results;
                Action::None
            }
            Msg::CycleMode => {
                self.mode = self.mode.next();
                // Re-run under the new mode if there is a query to run.
                if self.query.trim().is_empty() {
                    Action::None
                } else {
                    Action::Search(self.query.trim().to_owned(), self.mode)
                }
            }
            _ => Action::None,
        }
    }

    fn update_results(&mut self, msg: Msg) -> Action {
        match msg {
            Msg::FocusQuery => {
                self.focus = Focus::Query;
                Action::None
            }
            Msg::Blur => {
                // Esc closes an open note, else drops focus back to the query.
                if self.opened.is_some() {
                    self.opened = None;
                    self.note_scroll = 0;
                } else {
                    self.focus = Focus::Query;
                }
                Action::None
            }
            Msg::Down => {
                self.move_selection(1);
                Action::None
            }
            Msg::Up => {
                self.move_selection(-1);
                Action::None
            }
            Msg::ScrollDown => {
                self.note_scroll = self.note_scroll.saturating_add(SCROLL_STEP);
                Action::None
            }
            Msg::ScrollUp => {
                self.note_scroll = self.note_scroll.saturating_sub(SCROLL_STEP);
                Action::None
            }
            Msg::CycleMode => {
                self.mode = self.mode.next();
                if self.query.trim().is_empty() {
                    Action::None
                } else {
                    Action::Search(self.query.trim().to_owned(), self.mode)
                }
            }
            Msg::Open => match self.selected_hit() {
                Some(hit) => Action::Open(hit.id.clone()),
                None => Action::None,
            },
            Msg::Related => match self.selected_hit() {
                Some(hit) => Action::Related(hit.id.clone()),
                None => Action::None,
            },
            Msg::Submit => self.run_search(),
            _ => Action::None,
        }
    }

    fn run_search(&mut self) -> Action {
        let q = self.query.trim();
        if q.is_empty() {
            self.status = Some("type a query, then Enter".to_owned());
            Action::None
        } else {
            Action::Search(q.to_owned(), self.mode)
        }
    }

    fn move_selection(&mut self, delta: i32) {
        if self.results.is_empty() {
            self.selected = 0;
            return;
        }
        let max = (self.results.len() - 1) as i32;
        let next = (self.selected as i32 + delta).clamp(0, max) as usize;
        if next != self.selected {
            self.selected = next;
            self.opened = None; // moving off the note closes the preview
            self.note_scroll = 0;
        }
    }

    // --- setters the effect loop drives ---

    /// Install fresh hits and a status line, moving focus to the results.
    pub fn set_results(&mut self, hits: Vec<Hit>, status: impl Into<String>) {
        self.results = hits;
        self.selected = 0;
        self.opened = None;
        self.note_scroll = 0;
        self.status = Some(status.into());
        self.focus = Focus::Results;
    }

    /// Show a loaded note in the preview pane.
    pub fn set_opened(&mut self, note: OpenedNote) {
        self.opened = Some(note);
        self.note_scroll = 0;
    }

    pub fn set_flash(&mut self, flash: Flash) {
        self.flash = Some(flash);
    }
}

// --- view ---

/// Draw the Search screen into `area`.
pub fn render(frame: &mut Frame, app: &SearchApp, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);

    render_query(frame, app, rows[0]);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(rows[1]);
    render_results(frame, app, cols[0]);
    render_preview(frame, app, cols[1]);
}

fn render_query(frame: &mut Frame, app: &SearchApp, area: Rect) {
    let cursor = if app.is_typing() { "▍" } else { "" };
    let title = format!(" search · {} ", app.mode().label());
    let style = if app.is_typing() {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let body = Line::from(vec![
        Span::styled("› ", Style::default().fg(Color::DarkGray)),
        Span::raw(app.query().to_owned()),
        Span::styled(cursor, Style::default().fg(Color::Cyan)),
    ]);
    frame.render_widget(
        Paragraph::new(body).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(style),
        ),
        area,
    );
}

fn render_results(frame: &mut Frame, app: &SearchApp, area: Rect) {
    let title = match app.status() {
        Some(s) => format!(" results · {s} "),
        None => " results ".to_owned(),
    };
    if app.results().is_empty() {
        let hint =
            "Type a query and press Enter.\n\nModes: hybrid (default), vector, fts — cycle with m.";
        frame.render_widget(
            Paragraph::new(hint)
                .block(Block::default().borders(Borders::ALL).title(title))
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = app
        .results()
        .iter()
        .map(|h| {
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(
                        format!("{:>7.3} ", h.score),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::raw(h.title.clone()),
                ]),
                Line::from(Span::styled(
                    format!("        {}", h.path),
                    Style::default().fg(Color::DarkGray),
                )),
            ])
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▍");
    let mut state = ListState::default();
    state.select(Some(app.selected()));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_preview(frame: &mut Frame, app: &SearchApp, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" note ");
    if let Some(note) = app.opened() {
        let mut lines = vec![kv("title", &note.title), kv("path", &note.path)];
        if !note.tags.is_empty() {
            lines.push(kv("tags", &note.tags.join(", ")));
        }
        if let Some(src) = &note.source {
            lines.push(kv("source", src));
        }
        lines.push(kv("ingested", &note.ingested_at));
        lines.push(Line::from(""));
        for line in note.content.lines() {
            lines.push(Line::from(line.to_owned()));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .block(block)
                .wrap(Wrap { trim: false })
                .scroll((app.note_scroll(), 0)),
            area,
        );
        return;
    }
    match app.selected_hit() {
        Some(hit) => {
            let mut lines = vec![
                kv("title", &hit.title),
                kv("path", &hit.path),
                Line::from(""),
            ];
            if hit.snippet.is_empty() {
                lines.push(Line::from(Span::styled(
                    "(no excerpt)",
                    Style::default().fg(Color::DarkGray),
                )));
            } else {
                lines.push(Line::from(hit.snippet.clone()));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "press o / ↵ to open · r for related",
                Style::default().fg(Color::DarkGray),
            )));
            frame.render_widget(
                Paragraph::new(lines)
                    .block(block)
                    .wrap(Wrap { trim: false }),
                area,
            );
        }
        None => {
            frame.render_widget(
                Paragraph::new("No selection.")
                    .block(block)
                    .wrap(Wrap { trim: false }),
                area,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: &str, title: &str) -> Hit {
        Hit {
            score: 0.5,
            id: id.to_owned(),
            title: title.to_owned(),
            path: format!("{id}.md"),
            snippet: "excerpt".to_owned(),
        }
    }

    #[test]
    fn typing_then_enter_runs_a_search_in_the_current_mode() {
        let mut app = SearchApp::default();
        for c in "rust".chars() {
            assert_eq!(app.update(Msg::Char(c)), Action::None);
        }
        assert_eq!(app.query(), "rust");
        assert_eq!(
            app.update(Msg::Submit),
            Action::Search("rust".to_owned(), Mode::Hybrid)
        );
    }

    #[test]
    fn empty_query_submit_is_a_no_op_with_a_hint() {
        let mut app = SearchApp::default();
        assert_eq!(app.update(Msg::Submit), Action::None);
        assert!(app.status().unwrap().contains("type a query"));
    }

    #[test]
    fn backspace_edits_the_query() {
        let mut app = SearchApp::default();
        app.update(Msg::Char('a'));
        app.update(Msg::Char('b'));
        app.update(Msg::Backspace);
        assert_eq!(app.query(), "a");
    }

    #[test]
    fn results_navigation_open_and_related_use_the_selected_id() {
        let mut app = SearchApp::default();
        app.set_results(vec![hit("a", "A"), hit("b", "B")], "2 hits");
        assert!(!app.is_typing(), "results focus after a search");
        assert_eq!(app.selected(), 0);
        app.update(Msg::Down);
        assert_eq!(app.selected(), 1);
        assert_eq!(app.update(Msg::Open), Action::Open("b".to_owned()));
        assert_eq!(app.update(Msg::Related), Action::Related("b".to_owned()));
    }

    #[test]
    fn cycling_mode_reruns_only_with_a_query() {
        let mut app = SearchApp::default();
        // No query yet: cycle mode, no search.
        assert_eq!(app.update(Msg::CycleMode), Action::None);
        assert_eq!(app.mode(), Mode::Vector);
        for c in "db".chars() {
            app.update(Msg::Char(c));
        }
        assert_eq!(
            app.update(Msg::CycleMode),
            Action::Search("db".to_owned(), Mode::Fts)
        );
    }

    #[test]
    fn esc_closes_an_open_note_before_dropping_focus() {
        let mut app = SearchApp::default();
        app.set_results(vec![hit("a", "A")], "1 hit");
        app.set_opened(OpenedNote {
            id: "a".to_owned(),
            title: "A".to_owned(),
            path: "a.md".to_owned(),
            tags: vec![],
            source: None,
            ingested_at: "now".to_owned(),
            content: "body".to_owned(),
        });
        app.update(Msg::Blur); // first Esc closes the note
        assert!(app.opened().is_none());
        assert!(!app.is_typing(), "still on results");
        app.update(Msg::Blur); // second Esc drops to the query
        assert!(app.is_typing());
    }
}
