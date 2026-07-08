//! Rendering the reviewer: a proposal list, a detail/diff pane, a footer,
//! and the confirm / help overlays. Pure over [`ReviewApp`] + the loaded
//! detail — no I/O, no terminal state.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use curator_core::ProposalStatus;

use super::app::{FlashLevel, Loaded, Mode, Pending, ReviewApp, short_id, status_label};
use super::diff::diff_lines;

const EMPTY_STATE: &str = "No proposals.\n\n\
    Agents create proposals via `curator propose` or `curator digest run`.\n\
    They land here for you to review, apply, or reject.";

/// Draw the whole reviewer for one frame.
pub fn render(frame: &mut Frame, app: &ReviewApp, loaded: Option<&Loaded>) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(38), Constraint::Min(20)])
        .split(rows[0]);

    render_list(frame, app, cols[0]);
    render_detail(frame, app, loaded, cols[1]);
    render_footer(frame, app, rows[1]);

    match app.mode() {
        Mode::Confirm(pending) => render_confirm(frame, pending, loaded, frame.area()),
        Mode::Help => render_help(frame, frame.area()),
        Mode::Browse => {}
    }
}

fn render_list(frame: &mut Frame, app: &ReviewApp, area: Rect) {
    let visible = app.visible();
    let items: Vec<ListItem> = visible
        .iter()
        .map(|p| {
            let (glyph, color) = status_glyph(p.status);
            ListItem::new(Line::from(vec![
                Span::styled(format!("{glyph} "), Style::default().fg(color)),
                Span::raw(short_id(&p.id)),
                Span::raw("  "),
                Span::raw(p.title.clone()),
            ]))
        })
        .collect();

    let title = format!(" proposals · {} ({}) ", app.filter().label(), visible.len());
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

fn render_detail(frame: &mut Frame, app: &ReviewApp, loaded: Option<&Loaded>, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" detail ");

    if app.is_empty() {
        frame.render_widget(
            Paragraph::new(EMPTY_STATE)
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    if app.selected_proposal().is_none() {
        let msg = format!("no proposals match the '{}' filter", app.filter().label());
        frame.render_widget(
            Paragraph::new(msg).block(block).wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let Some(loaded) = loaded else {
        frame.render_widget(Paragraph::new("loading…").block(block), area);
        return;
    };

    let p = &loaded.proposal;
    let mut lines: Vec<Line> = vec![
        kv("title", &p.title),
        kv("author", &p.author),
        kv("created", &p.created),
        kv("status", status_label(p.status)),
    ];
    if !p.rationale.is_empty() {
        lines.push(kv("rationale", &p.rationale));
    }
    lines.push(Line::from(format!("files: {}", p.files.len())));
    lines.push(Line::from(""));

    // Pre-flight banner: only meaningful while the proposal is still open.
    if p.status == ProposalStatus::Open {
        if let Some(warn) = &loaded.preflight.warning {
            lines.push(Line::from(Span::styled(
                format!("⚠ {warn}"),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "✓ applies cleanly against the current vault",
                Style::default().fg(Color::Green),
            )));
        }
        lines.push(Line::from(""));
    }

    if let Some(err) = &loaded.parse_error {
        lines.push(Line::from(Span::styled(
            format!("patch parse error: {err}"),
            Style::default().fg(Color::Red),
        )));
    } else {
        lines.extend(diff_lines(&loaded.file_patches));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .scroll((app.diff_scroll(), 0)),
        area,
    );
}

fn render_footer(frame: &mut Frame, app: &ReviewApp, area: Rect) {
    let line = match app.flash() {
        Some(flash) => Line::from(Span::styled(
            format!(" {}", flash.text),
            Style::default()
                .fg(flash_color(flash.level))
                .add_modifier(Modifier::BOLD),
        )),
        None => Line::from(Span::styled(
            " j/k move · ^d/^u scroll · f filter · a apply · x reject · r refresh · ? help · q quit",
            Style::default().fg(Color::DarkGray),
        )),
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn render_confirm(frame: &mut Frame, pending: &Pending, loaded: Option<&Loaded>, area: Rect) {
    let (verb, id, is_apply) = match pending {
        Pending::Apply(id) => ("Apply", id, true),
        Pending::Reject(id) => ("Reject", id, false),
    };
    let mut lines = vec![Line::from(Span::styled(
        format!("{verb} proposal {}?", short_id(id)),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    if let Some(l) = loaded {
        lines.push(Line::from(l.proposal.title.clone()));
        if is_apply {
            if let Some(warn) = &l.preflight.warning {
                lines.push(Line::from(Span::styled(
                    format!("⚠ {warn}"),
                    Style::default().fg(Color::Yellow),
                )));
            }
            lines.push(Line::from(Span::styled(
                "apply writes the files; a failed apply is a terminal reject.",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            "[y]",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" confirm    "),
        Span::styled(
            "[n]",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" cancel"),
    ]));

    let popup = centered_rect(60, 34, area);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" confirm {} ", verb.to_lowercase())),
            )
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn render_help(frame: &mut Frame, area: Rect) {
    let keys = [
        ("j / k   ↑ / ↓", "move between proposals"),
        ("^d / ^u", "scroll the diff (also PgDn / PgUp)"),
        ("f", "cycle filter: all → open → applied → rejected"),
        ("a", "apply the selected proposal (asks to confirm)"),
        ("x", "reject the selected proposal (asks to confirm)"),
        ("r", "refresh the queue from disk"),
        ("?", "toggle this help"),
        ("q / Esc", "quit"),
    ];
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            "curator review — keys",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    for (k, v) in keys {
        lines.push(Line::from(vec![
            Span::styled(format!("  {k:<14}"), Style::default().fg(Color::Cyan)),
            Span::raw(v),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "any key to dismiss",
        Style::default().fg(Color::DarkGray),
    )));

    let popup = centered_rect(66, 60, area);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" help "))
            .wrap(Wrap { trim: false }),
        popup,
    );
}

// --- small helpers ---

fn kv(key: &str, val: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key}: "), Style::default().fg(Color::DarkGray)),
        Span::raw(val.to_owned()),
    ])
}

fn status_glyph(status: ProposalStatus) -> (&'static str, Color) {
    match status {
        ProposalStatus::Open => ("●", Color::Yellow),
        ProposalStatus::Applied => ("✓", Color::Green),
        ProposalStatus::Rejected => ("✗", Color::Red),
    }
}

fn flash_color(level: FlashLevel) -> Color {
    match level {
        FlashLevel::Success => Color::Green,
        FlashLevel::Warn => Color::Yellow,
        FlashLevel::Error => Color::Red,
    }
}

/// A rectangle `pct_x` × `pct_y` percent of `area`, centred.
fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(vert[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use curator_core::Proposal;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn open_proposal() -> Proposal {
        Proposal {
            schema: "proposals/v1".to_owned(),
            id: "01HELLOWORLD".to_owned(),
            created: "2026-07-07T00:00:00Z".to_owned(),
            author: "kp-librarian".to_owned(),
            title: "Daily digest 2026-07-07".to_owned(),
            rationale: "surfaced".to_owned(),
            status: ProposalStatus::Open,
            files: vec!["digests/2026-07-07.md".to_owned()],
        }
    }

    fn rendered_text(app: &ReviewApp, loaded: Option<&Loaded>) -> String {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|f| render(f, app, loaded)).expect("draw");
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    #[test]
    fn renders_the_list_title_and_footer_hints() {
        let app = ReviewApp::new(vec![open_proposal()]);
        let text = rendered_text(&app, None);
        assert!(text.contains("Daily digest"), "list shows the title");
        assert!(text.contains("apply"), "footer shows key hints");
        assert!(text.contains("proposals"), "list block title present");
    }

    #[test]
    fn empty_queue_shows_the_empty_state() {
        let app = ReviewApp::new(vec![]);
        let text = rendered_text(&app, None);
        assert!(text.contains("No proposals"), "got: {text}");
    }
}
