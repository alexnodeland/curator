//! Strict unified-diff parsing and application for `changes.patch`.
//!
//! `proposals/v1` stores a proposal's changes as a unified diff against
//! the vault working tree (rendered by curator-core via the `similar` crate).
//! Applying is the security-relevant half: context and deletion lines
//! must match the current file EXACTLY — there is no fuzz, no offset
//! search, no partial application. Anything that does not line up is a
//! contract hard-reject (`patches that do not apply cleanly`), because a
//! drifted vault means the human reviewed a change that is no longer the
//! change.
//!
//! The dialect parsed here is exactly what curator-core emits: `--- a/<path>`
//! (or `/dev/null` for creations), `+++ b/<path>`, `@@ -s[,n] +s[,n] @@`
//! hunks, and `\ No newline at end of file` markers.

/// One line inside a hunk. The `bool` is "has a trailing newline" —
/// `false` only for a final line flagged by the no-newline marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HunkLine {
    Context(String, bool),
    Add(String, bool),
    Remove(String, bool),
}

/// One `@@` hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// 1-based first old line (0 with `old_len == 0` = pure insertion
    /// before line 1).
    pub old_start: usize,
    pub old_len: usize,
    pub new_start: usize,
    pub new_len: usize,
    pub lines: Vec<HunkLine>,
}

/// One file's worth of a `changes.patch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePatch {
    /// `None` = creation (`--- /dev/null`).
    pub old_path: Option<String>,
    /// Vault-relative target path (from `+++ b/<path>`).
    pub new_path: String,
    pub hunks: Vec<Hunk>,
}

/// Errors from parsing or applying a patch.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PatchError {
    /// The patch text is not the dialect curator-core emits.
    #[error("malformed patch at line {line}: {reason}")]
    Malformed { line: usize, reason: String },
    /// The patch does not apply cleanly to the current file content.
    #[error("patch does not apply cleanly to {path}: {reason}")]
    NonClean { path: String, reason: String },
}

fn malformed(line: usize, reason: impl Into<String>) -> PatchError {
    PatchError::Malformed {
        line,
        reason: reason.into(),
    }
}

/// Parse a whole `changes.patch` into per-file patches.
pub fn parse_patch(patch: &str) -> Result<Vec<FilePatch>, PatchError> {
    let mut files: Vec<FilePatch> = Vec::new();
    let mut lines = patch.lines().enumerate().peekable();

    while let Some((n, line)) = lines.next() {
        let lineno = n + 1;
        let Some(old_header) = line.strip_prefix("--- ") else {
            return Err(malformed(lineno, format!("expected `--- `, got {line:?}")));
        };
        let old_path = match old_header {
            "/dev/null" => None,
            other => Some(
                other
                    .strip_prefix("a/")
                    .ok_or_else(|| malformed(lineno, "old header must be /dev/null or a/<path>"))?
                    .to_owned(),
            ),
        };
        let (n, line) = lines
            .next()
            .ok_or_else(|| malformed(lineno + 1, "missing +++ header"))?;
        let new_path = line
            .strip_prefix("+++ b/")
            .ok_or_else(|| malformed(n + 1, "new header must be +++ b/<path>"))?
            .to_owned();

        let mut hunks = Vec::new();
        while let Some((_, peek)) = lines.peek() {
            if !peek.starts_with("@@") {
                break;
            }
            let (n, header) = lines.next().expect("peeked");
            let (old_start, old_len, new_start, new_len) = parse_hunk_header(header, n + 1)?;
            let mut body = Vec::new();
            let (mut seen_old, mut seen_new) = (0usize, 0usize);
            while seen_old < old_len || seen_new < new_len {
                let (n, raw) = lines
                    .next()
                    .ok_or_else(|| malformed(0, "patch truncated inside a hunk"))?;
                let lineno = n + 1;
                match raw.as_bytes().first() {
                    Some(b' ') => {
                        body.push(HunkLine::Context(raw[1..].to_owned(), true));
                        seen_old += 1;
                        seen_new += 1;
                    }
                    Some(b'-') => {
                        body.push(HunkLine::Remove(raw[1..].to_owned(), true));
                        seen_old += 1;
                    }
                    Some(b'+') => {
                        body.push(HunkLine::Add(raw[1..].to_owned(), true));
                        seen_new += 1;
                    }
                    Some(b'\\') => strip_last_newline(&mut body, lineno)?,
                    _ => return Err(malformed(lineno, format!("unexpected hunk line {raw:?}"))),
                }
            }
            // A trailing no-newline marker after the counted lines.
            if let Some((_, peek)) = lines.peek()
                && peek.starts_with('\\')
            {
                let (n, _) = lines.next().expect("peeked");
                strip_last_newline(&mut body, n + 1)?;
            }
            hunks.push(Hunk {
                old_start,
                old_len,
                new_start,
                new_len,
                lines: body,
            });
        }
        if hunks.is_empty() {
            return Err(malformed(lineno, format!("no hunks for {new_path}")));
        }
        files.push(FilePatch {
            old_path,
            new_path,
            hunks,
        });
    }
    Ok(files)
}

/// `\ No newline at end of file`: flags the previous diff line.
fn strip_last_newline(body: &mut [HunkLine], lineno: usize) -> Result<(), PatchError> {
    match body.last_mut() {
        Some(HunkLine::Context(_, nl) | HunkLine::Add(_, nl) | HunkLine::Remove(_, nl)) => {
            *nl = false;
            Ok(())
        }
        None => Err(malformed(
            lineno,
            "no-newline marker with no preceding line",
        )),
    }
}

/// `@@ -<start>[,<len>] +<start>[,<len>] @@` (lengths default to 1).
fn parse_hunk_header(
    header: &str,
    lineno: usize,
) -> Result<(usize, usize, usize, usize), PatchError> {
    let bad = || malformed(lineno, format!("bad hunk header {header:?}"));
    let rest = header.strip_prefix("@@ -").ok_or_else(bad)?;
    let (old_part, rest) = rest.split_once(" +").ok_or_else(bad)?;
    let (new_part, _) = rest.split_once(" @@").ok_or_else(bad)?;
    let parse_range = |part: &str| -> Option<(usize, usize)> {
        match part.split_once(',') {
            Some((s, l)) => Some((s.parse().ok()?, l.parse().ok()?)),
            None => Some((part.parse().ok()?, 1)),
        }
    };
    let (old_start, old_len) = parse_range(old_part).ok_or_else(bad)?;
    let (new_start, new_len) = parse_range(new_part).ok_or_else(bad)?;
    Ok((old_start, old_len, new_start, new_len))
}

/// One line of real file content: text without its newline + whether one
/// followed.
#[derive(Debug, PartialEq, Eq)]
struct ContentLine<'a> {
    text: &'a str,
    newline: bool,
}

fn split_lines(content: &str) -> Vec<ContentLine<'_>> {
    content
        .split_inclusive('\n')
        .map(|raw| match raw.strip_suffix('\n') {
            Some(text) => ContentLine {
                text,
                newline: true,
            },
            None => ContentLine {
                text: raw,
                newline: false,
            },
        })
        .collect()
}

/// Apply one file's hunks to its current content, STRICTLY: every context
/// and deletion line must match the file exactly (text and trailing
/// newline both). Returns the new content.
pub fn apply_file_patch(old: &str, patch: &FilePatch) -> Result<String, PatchError> {
    let path = &patch.new_path;
    let non_clean = |reason: String| PatchError::NonClean {
        path: path.clone(),
        reason,
    };
    let old_lines = split_lines(old);
    let mut out = String::new();
    let mut cursor = 0usize; // 0-based index into old_lines

    for hunk in &patch.hunks {
        // Unified-diff convention: a zero-length old range names the line
        // AFTER which to insert; a non-empty range names its first line.
        let hunk_pos = if hunk.old_len == 0 {
            hunk.old_start
        } else {
            hunk.old_start
                .checked_sub(1)
                .ok_or_else(|| non_clean("hunk old range starts at line 0".to_owned()))?
        };
        if hunk_pos < cursor {
            return Err(non_clean("hunks overlap or are out of order".to_owned()));
        }
        if hunk_pos > old_lines.len() {
            return Err(non_clean(format!(
                "hunk targets line {} but the file has {} line(s)",
                hunk.old_start,
                old_lines.len()
            )));
        }
        for line in &old_lines[cursor..hunk_pos] {
            push_line(&mut out, line.text, line.newline);
        }
        cursor = hunk_pos;

        for hunk_line in &hunk.lines {
            match hunk_line {
                HunkLine::Add(text, nl) => push_line(&mut out, text, *nl),
                HunkLine::Context(text, nl) | HunkLine::Remove(text, nl) => {
                    let actual = old_lines.get(cursor).ok_or_else(|| {
                        non_clean(format!(
                            "expected {text:?} at line {} but the file ended",
                            cursor + 1
                        ))
                    })?;
                    if actual.text != text || actual.newline != *nl {
                        return Err(non_clean(format!(
                            "line {} is {:?}, patch expects {text:?}",
                            cursor + 1,
                            actual.text
                        )));
                    }
                    if matches!(hunk_line, HunkLine::Context(..)) {
                        push_line(&mut out, text, *nl);
                    }
                    cursor += 1;
                }
            }
        }
    }
    for line in &old_lines[cursor..] {
        push_line(&mut out, line.text, line.newline);
    }
    Ok(out)
}

fn push_line(out: &mut String, text: &str, newline: bool) {
    out.push_str(text);
    if newline {
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use curator_core::{ProposalFile, Vault, create_proposal};

    /// Render a patch exactly the way curator-core does, via a real proposal.
    fn curator_core_patch(existing: &[(&str, &str)], proposed: &[(&str, &str)]) -> String {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("vault");
        std::fs::create_dir_all(&root).expect("mkdir");
        let vault = Vault::open(&root).expect("open");
        for (path, content) in existing {
            vault.write_atomic(path, content).expect("seed");
        }
        let files: Vec<ProposalFile> = proposed
            .iter()
            .map(|(path, content)| ProposalFile {
                path: (*path).to_owned(),
                content: (*content).to_owned(),
            })
            .collect();
        let p = create_proposal(&vault, ".kp/proposals", "t", "t", "r", &files).expect("creates");
        vault
            .read(&format!(".kp/proposals/{}/changes.patch", p.id))
            .expect("patch")
    }

    #[test]
    fn round_trips_a_creation() {
        let patch = curator_core_patch(&[], &[("notes/new.md", "# Title\n\nbody line\n")]);
        let files = parse_patch(&patch).expect("parses");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].old_path, None);
        assert_eq!(files[0].new_path, "notes/new.md");
        let out = apply_file_patch("", &files[0]).expect("applies");
        assert_eq!(out, "# Title\n\nbody line\n");
    }

    #[test]
    fn round_trips_a_modification() {
        let old = "one\ntwo\nthree\nfour\nfive\n";
        let new = "one\n2\nthree\nfour\nfive\nsix\n";
        let patch = curator_core_patch(&[("n.md", old)], &[("n.md", new)]);
        let files = parse_patch(&patch).expect("parses");
        assert_eq!(files[0].old_path.as_deref(), Some("n.md"));
        assert_eq!(apply_file_patch(old, &files[0]).expect("applies"), new);
    }

    #[test]
    fn round_trips_multiple_files_and_hunks() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\n";
        let new = "a\nB\nc\nd\ne\nf\ng\nh\ni\nj\nK\nl\n";
        let patch = curator_core_patch(
            &[("x.md", old)],
            &[("x.md", new), ("fresh.md", "created\n")],
        );
        let files = parse_patch(&patch).expect("parses");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].hunks.len(), 2, "two separated edits, two hunks");
        assert_eq!(apply_file_patch(old, &files[0]).expect("applies"), new);
        assert_eq!(
            apply_file_patch("", &files[1]).expect("applies"),
            "created\n"
        );
    }

    #[test]
    fn round_trips_missing_trailing_newlines() {
        // New side loses the newline…
        let patch = curator_core_patch(&[("n.md", "one\n")], &[("n.md", "one\ntwo")]);
        let files = parse_patch(&patch).expect("parses");
        assert_eq!(
            apply_file_patch("one\n", &files[0]).expect("applies"),
            "one\ntwo"
        );
        // …and the old side lacked one.
        let patch = curator_core_patch(&[("n.md", "one\ntwo")], &[("n.md", "one\ntwo\nthree\n")]);
        let files = parse_patch(&patch).expect("parses");
        assert_eq!(
            apply_file_patch("one\ntwo", &files[0]).expect("applies"),
            "one\ntwo\nthree\n"
        );
    }

    #[test]
    fn drifted_content_is_non_clean() {
        let old = "one\ntwo\nthree\n";
        let patch = curator_core_patch(&[("n.md", old)], &[("n.md", "one\n2\nthree\n")]);
        let files = parse_patch(&patch).expect("parses");
        // The vault moved on since the proposal was created.
        let err = apply_file_patch("one\nTWO\nthree\n", &files[0]).unwrap_err();
        assert!(matches!(err, PatchError::NonClean { ref path, .. } if path == "n.md"));
        // Shorter file: context runs off the end.
        let err = apply_file_patch("one\n", &files[0]).unwrap_err();
        assert!(matches!(err, PatchError::NonClean { .. }));
        // A newline-presence mismatch is drift too.
        let err = apply_file_patch("one\ntwo\nthree", &files[0]).unwrap_err();
        assert!(matches!(err, PatchError::NonClean { .. }));
    }

    #[test]
    fn malformed_patches_are_rejected() {
        for (bad, what) in [
            ("not a patch\n", "missing ---"),
            ("--- a/x.md\nmissing plus header\n", "bad +++"),
            ("--- a/x.md\n+++ b/x.md\n", "no hunks"),
            ("--- a/x.md\n+++ b/x.md\n@@ nonsense @@\n x\n", "bad header"),
            (
                "--- a/x.md\n+++ b/x.md\n@@ -1,2 +1,2 @@\n one\n",
                "truncated",
            ),
            (
                "--- x.md\n+++ b/x.md\n@@ -1 +1 @@\n-a\n+b\n",
                "old header prefix",
            ),
        ] {
            assert!(
                matches!(parse_patch(bad), Err(PatchError::Malformed { .. })),
                "{what}: {bad:?}"
            );
        }
    }

    #[test]
    fn insertion_at_top_of_file() {
        let old = "body\n";
        let new = "header\nbody\n";
        let patch = curator_core_patch(&[("n.md", old)], &[("n.md", new)]);
        let files = parse_patch(&patch).expect("parses");
        assert_eq!(apply_file_patch(old, &files[0]).expect("applies"), new);
    }
}
