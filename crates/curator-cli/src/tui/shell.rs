//! The tabbed shell around the three screens (Review · Search · Digest).
//!
//! It owns the tab bar, the footer, the global help overlay, and the global
//! key routing: `Tab`/`Shift-Tab` and `Ctrl-C` are global at all times; the
//! rest of the global shortcuts (`q`, `?`, `1`/`2`/`3`) fire only when the
//! active screen is not *modal* — i.e. not capturing input (the Search query
//! box) or showing an overlay (a Review/Digest confirm). A modal screen gets
//! every other key forwarded to it.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::app::ReviewApp;
use super::common::{Flash, centered_rect, flash_color};
use super::digest::DigestApp;
use super::search::SearchApp;

/// The three screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Review,
    Search,
    Digest,
}

impl Tab {
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Tab::Review => "Review",
            Tab::Search => "Search",
            Tab::Digest => "Digest",
        }
    }
    fn next(self) -> Self {
        match self {
            Tab::Review => Tab::Search,
            Tab::Search => Tab::Digest,
            Tab::Digest => Tab::Review,
        }
    }
    fn prev(self) -> Self {
        match self {
            Tab::Review => Tab::Digest,
            Tab::Search => Tab::Review,
            Tab::Digest => Tab::Search,
        }
    }
    fn from_digit(c: char) -> Option<Self> {
        match c {
            '1' => Some(Tab::Review),
            '2' => Some(Tab::Search),
            '3' => Some(Tab::Digest),
            _ => None,
        }
    }
    fn order() -> [Tab; 3] {
        [Tab::Review, Tab::Search, Tab::Digest]
    }
}

/// A global action decoded from a key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalMsg {
    Quit,
    ToggleHelp,
    Switch(Tab),
}

/// The whole app: the active tab, the three screen states, and whether the
/// global help overlay is open.
#[derive(Debug)]
pub struct Shell {
    pub active: Tab,
    pub review: ReviewApp,
    pub search: SearchApp,
    pub digest: DigestApp,
    pub help: bool,
}

impl Shell {
    #[must_use]
    pub fn new(review: ReviewApp) -> Self {
        Self {
            active: Tab::Review,
            review,
            search: SearchApp::default(),
            digest: DigestApp::default(),
            help: false,
        }
    }

    /// The active screen is capturing input / showing an overlay — the loop
    /// forwards all non-`Tab`/`Ctrl-C` keys to it.
    #[must_use]
    pub fn active_is_modal(&self) -> bool {
        match self.active {
            Tab::Review => self.review.is_modal(),
            Tab::Search => self.search.is_typing(),
            Tab::Digest => self.digest.is_modal(),
        }
    }

    fn active_flash(&self) -> Option<&Flash> {
        match self.active {
            Tab::Review => self.review.flash(),
            Tab::Search => self.search.flash(),
            Tab::Digest => self.digest.flash(),
        }
    }

    fn active_hint(&self) -> &'static str {
        match self.active {
            Tab::Review => super::view::HINT,
            Tab::Search => super::search::HINT,
            Tab::Digest => super::digest::HINT,
        }
    }
}

/// Decode a key into a [`GlobalMsg`], or `None` to forward it to the active
/// screen. `Tab`/`Shift-Tab`/`Ctrl-C` are always global; the rest apply only
/// when the active screen is not modal.
#[must_use]
pub fn decode_global(key: KeyEvent, active: Tab, modal: bool) -> Option<GlobalMsg> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('c') if ctrl => Some(GlobalMsg::Quit),
        KeyCode::Tab => Some(GlobalMsg::Switch(active.next())),
        KeyCode::BackTab => Some(GlobalMsg::Switch(active.prev())),
        _ if modal => None,
        // `q` quits from any non-modal screen; `Esc` is left for the screens
        // to use as back / cancel / close-preview.
        KeyCode::Char('q') => Some(GlobalMsg::Quit),
        KeyCode::Char('?') => Some(GlobalMsg::ToggleHelp),
        KeyCode::Char(c @ '1'..='3') => Tab::from_digit(c).map(GlobalMsg::Switch),
        _ => None,
    }
}

/// Draw the whole frame: tab bar, active screen, footer, and any overlay.
pub fn render(frame: &mut Frame, shell: &Shell, review_loaded: Option<&super::app::Loaded>) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    render_tabs(frame, shell.active, rows[0]);
    match shell.active {
        Tab::Review => super::view::render(frame, &shell.review, review_loaded, rows[1]),
        Tab::Search => super::search::render(frame, &shell.search, rows[1]),
        Tab::Digest => super::digest::render(frame, &shell.digest, rows[1]),
    }
    render_footer(frame, shell, rows[2]);

    if shell.help {
        render_help(frame, frame.area());
    }
}

fn render_tabs(frame: &mut Frame, active: Tab, area: Rect) {
    let mut spans = vec![Span::raw(" ")];
    for (i, tab) in Tab::order().into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
        }
        let label = format!(" {} ", tab.title());
        let style = if tab == active {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(label, style));
    }
    spans.push(Span::styled(
        "   Tab/1·2·3 switch · ? help",
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_footer(frame: &mut Frame, shell: &Shell, area: Rect) {
    let line = match shell.active_flash() {
        Some(flash) => Line::from(Span::styled(
            format!(" {}", flash.text),
            Style::default()
                .fg(flash_color(flash.level))
                .add_modifier(Modifier::BOLD),
        )),
        None => Line::from(vec![
            Span::styled(shell.active_hint(), Style::default().fg(Color::DarkGray)),
            Span::styled(" · q quit", Style::default().fg(Color::DarkGray)),
        ]),
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn render_help(frame: &mut Frame, area: Rect) {
    let section = |title: &'static str| {
        Line::from(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let key = |k: &str, v: &str| {
        Line::from(vec![
            Span::styled(format!("  {k:<12}"), Style::default().fg(Color::Gray)),
            Span::raw(v.to_owned()),
        ])
    };
    let lines = vec![
        Line::from(Span::styled(
            "curator — keys",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        section("global"),
        key("Tab / 1·2·3", "switch screen (Review · Search · Digest)"),
        key("?", "toggle this help · Esc: back / cancel"),
        key("q", "quit (Ctrl-C anywhere)"),
        Line::from(""),
        section("Review"),
        key("j / k", "move · ^d/^u scroll the diff"),
        key("f", "filter · a apply · x reject · r refresh"),
        Line::from(""),
        section("Search"),
        key("type · ↵", "edit the query · run it"),
        key("j / k", "move · o open · r related · m mode · / edit"),
        Line::from(""),
        section("Digest"),
        key("j / k", "move · ^d/^u scroll · f filter"),
        key("g", "generate today's digest · r refresh"),
        Line::from(""),
        Line::from(Span::styled(
            "any key to dismiss",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let popup = centered_rect(72, 80, area);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" help "))
            .wrap(Wrap { trim: false }),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEventKind;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn tab_and_backtab_cycle_regardless_of_modality() {
        // Even a modal screen yields Tab.
        assert_eq!(
            decode_global(press(KeyCode::Tab), Tab::Review, true),
            Some(GlobalMsg::Switch(Tab::Search))
        );
        assert_eq!(
            decode_global(press(KeyCode::BackTab), Tab::Review, false),
            Some(GlobalMsg::Switch(Tab::Digest))
        );
    }

    #[test]
    fn shortcuts_are_suppressed_while_modal() {
        // Not modal: q quits, digits switch.
        assert_eq!(
            decode_global(press(KeyCode::Char('q')), Tab::Search, false),
            Some(GlobalMsg::Quit)
        );
        assert_eq!(
            decode_global(press(KeyCode::Char('3')), Tab::Review, false),
            Some(GlobalMsg::Switch(Tab::Digest))
        );
        // Modal (e.g. typing a query): q and digits are the screen's, not global.
        assert_eq!(
            decode_global(press(KeyCode::Char('q')), Tab::Search, true),
            None
        );
        assert_eq!(
            decode_global(press(KeyCode::Char('3')), Tab::Search, true),
            None
        );
    }

    #[test]
    fn ctrl_c_quits_even_while_modal() {
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(decode_global(key, Tab::Search, true), Some(GlobalMsg::Quit));
    }

    #[test]
    fn key_event_kind_is_available_for_press_filtering() {
        // Guards against a crossterm skew where key.kind vanishes.
        let k = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(k.kind, KeyEventKind::Press);
    }
}
