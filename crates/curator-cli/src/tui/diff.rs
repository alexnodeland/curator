//! Colouring the structured proposal diff for the review pane.
//!
//! Consumes the already-parsed [`FilePatch`]es (from `curator_librarian::
//! parse_patch`) and emits styled ratatui lines: a per-file header, hunk
//! markers, and `+`/`-` coloured body lines. Owns its strings (`'static`)
//! so the view can build it fresh per selection.

use curator_librarian::{FilePatch, HunkLine};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Render parsed file patches as styled diff lines.
#[must_use]
pub fn diff_lines(file_patches: &[FilePatch]) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    for fp in file_patches {
        // `old_path: None` is a creation; `Some(_)` is a whole-file replace
        // (the proposals/v1 model has no delete or rename).
        let kind = if fp.old_path.is_none() {
            "create"
        } else {
            "modify"
        };
        out.push(Line::from(Span::styled(
            format!("{kind}  {}", fp.new_path),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        for hunk in &fp.hunks {
            out.push(Line::from(Span::styled(
                format!(
                    "@@ -{},{} +{},{} @@",
                    hunk.old_start, hunk.old_len, hunk.new_start, hunk.new_len
                ),
                Style::default().fg(Color::DarkGray),
            )));
            for line in &hunk.lines {
                let (sign, text, style) = match line {
                    HunkLine::Add(t, _) => ('+', t, Style::default().fg(Color::Green)),
                    HunkLine::Remove(t, _) => ('-', t, Style::default().fg(Color::Red)),
                    HunkLine::Context(t, _) => (' ', t, Style::default().fg(Color::Gray)),
                };
                out.push(Line::from(Span::styled(format!("{sign}{text}"), style)));
            }
        }
        out.push(Line::from("")); // a blank line between files
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use curator_librarian::{Hunk, HunkLine};

    fn text_of(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn labels_creates_and_modifies_and_signs_lines() {
        let create = FilePatch {
            old_path: None,
            new_path: "a.md".to_owned(),
            hunks: vec![Hunk {
                old_start: 0,
                old_len: 0,
                new_start: 1,
                new_len: 1,
                lines: vec![HunkLine::Add("hello".to_owned(), true)],
            }],
        };
        let modify = FilePatch {
            old_path: Some("b.md".to_owned()),
            new_path: "b.md".to_owned(),
            hunks: vec![Hunk {
                old_start: 1,
                old_len: 1,
                new_start: 1,
                new_len: 1,
                lines: vec![
                    HunkLine::Remove("old".to_owned(), true),
                    HunkLine::Add("new".to_owned(), true),
                    HunkLine::Context("kept".to_owned(), true),
                ],
            }],
        };
        let rendered = text_of(&diff_lines(&[create, modify]));
        assert!(rendered.contains("create  a.md"), "{rendered}");
        assert!(rendered.contains("modify  b.md"), "{rendered}");
        assert!(rendered.contains("+hello"));
        assert!(rendered.contains("-old"));
        assert!(rendered.contains("+new"));
        assert!(rendered.contains(" kept"));
    }
}
