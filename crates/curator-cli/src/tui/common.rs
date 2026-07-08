//! Small helpers shared across the reviewer's screens: the footer flash,
//! centred-overlay geometry, and the `key: value` metadata line. Pure — no
//! terminal state, no I/O.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// A short-lived status line shown in the footer after an action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flash {
    pub level: FlashLevel,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashLevel {
    Success,
    Warn,
    Error,
}

impl Flash {
    pub fn success(text: impl Into<String>) -> Self {
        Self {
            level: FlashLevel::Success,
            text: text.into(),
        }
    }
    pub fn warn(text: impl Into<String>) -> Self {
        Self {
            level: FlashLevel::Warn,
            text: text.into(),
        }
    }
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            level: FlashLevel::Error,
            text: text.into(),
        }
    }
}

#[must_use]
pub fn flash_color(level: FlashLevel) -> Color {
    match level {
        FlashLevel::Success => Color::Green,
        FlashLevel::Warn => Color::Yellow,
        FlashLevel::Error => Color::Red,
    }
}

/// A `key: value` line — dark-grey label, default-fg value.
#[must_use]
pub fn kv(key: &str, val: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key}: "), Style::default().fg(Color::DarkGray)),
        Span::raw(val.to_owned()),
    ])
}

/// A rectangle `pct_x` × `pct_y` percent of `area`, centred — for overlays.
#[must_use]
pub fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
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
