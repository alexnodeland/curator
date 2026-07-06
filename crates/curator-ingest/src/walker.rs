//! The vault walker: every note the plane should look at, and nothing else.
//!
//! Built on [`Vault::note_paths`], which already skips dot-entries —
//! `.kp/`, `.curio/`, `.git/`, and every other dotfile are machinery, not
//! notes. On top of that this module honors an optional **`.kpignore`**
//! file at the vault root:
//!
//! - one pattern per line; blank lines and `#` comments are skipped;
//! - `*` matches within one path segment, `**` matches any number of
//!   segments, `?` matches one character;
//! - a trailing `/` ignores everything under matching directories;
//! - a pattern containing a `/` is anchored at the vault root; a pattern
//!   without one matches the file name (or, with a trailing `/`, the
//!   directory name) at any depth;
//! - no negation (`!`) in v1 — deliberately small.
//!
//! Unparseable notes are skipped with a warning, never fatal: one
//! malformed producer file must not brick an ingest run.

use curator_core::{Note, Vault, VaultError};

/// A note the walker read and parsed.
#[derive(Debug, Clone)]
pub struct WalkedNote {
    /// Vault-relative path, forward slashes.
    pub rel_path: String,
    /// The note exactly as parsed from disk.
    pub note: Note,
}

/// What one vault walk saw.
#[derive(Debug, Default)]
pub struct WalkReport {
    /// Parsed notes, path-sorted.
    pub notes: Vec<WalkedNote>,
    /// `(path, warning)` for files that failed to parse — skipped.
    pub skipped: Vec<(String, String)>,
    /// Files dropped by `.kpignore`.
    pub ignored: Vec<String>,
}

/// Walk the vault: every `.md` note, minus `.kpignore` matches, parsed.
pub fn walk_vault(vault: &Vault) -> Result<WalkReport, VaultError> {
    let ignore = load_kpignore(vault);
    let mut report = WalkReport::default();
    for rel in vault.note_paths()? {
        if ignore.matches(&rel) {
            report.ignored.push(rel);
            continue;
        }
        match vault.read_note(&rel) {
            Ok(note) => report.notes.push(WalkedNote {
                rel_path: rel,
                note,
            }),
            Err(err) => {
                tracing::warn!(note = %rel, %err, "skipping unparseable note");
                report.skipped.push((rel, err.to_string()));
            }
        }
    }
    Ok(report)
}

/// Load `.kpignore` from the vault root; a missing file is an empty set.
fn load_kpignore(vault: &Vault) -> KpIgnore {
    match vault.read(".kpignore") {
        Ok(raw) => KpIgnore::parse(&raw),
        Err(_) => KpIgnore::default(),
    }
}

/// A parsed `.kpignore` pattern set.
#[derive(Debug, Default, Clone)]
pub struct KpIgnore {
    /// Normalized patterns: segment vectors matched against relpaths.
    patterns: Vec<Vec<String>>,
}

impl KpIgnore {
    /// Parse `.kpignore` content (see module docs for the semantics).
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        let mut patterns = Vec::new();
        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (body, dir) = match line.strip_suffix('/') {
                Some(b) => (b, true),
                None => (line, false),
            };
            let body = body.trim_matches('/');
            if body.is_empty() {
                continue;
            }
            let mut segs: Vec<String> = body.split('/').map(str::to_owned).collect();
            // A directory pattern swallows everything below it.
            if dir {
                segs.push("**".to_owned());
            }
            // No `/` in the original pattern → match at any depth.
            if !body.contains('/') {
                segs.insert(0, "**".to_owned());
            }
            patterns.push(segs);
        }
        Self { patterns }
    }

    /// Does any pattern match this vault-relative path?
    #[must_use]
    pub fn matches(&self, rel_path: &str) -> bool {
        let path: Vec<&str> = rel_path.split('/').collect();
        self.patterns
            .iter()
            .any(|p| segs_match(&seg_refs(p), &path))
    }
}

fn seg_refs(p: &[String]) -> Vec<&str> {
    p.iter().map(String::as_str).collect()
}

/// Match pattern segments against path segments; `**` spans any number of
/// path segments.
fn segs_match(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segs_match(rest, &path[skip..])),
        Some((first, rest)) => match path.split_first() {
            Some((seg, path_rest)) => glob_match(first, seg) && segs_match(rest, path_rest),
            None => false,
        },
    }
}

/// Single-segment glob: `*` = any run, `?` = one char, else literal.
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_at(&p, &t)
}

fn glob_at(p: &[char], t: &[char]) -> bool {
    match p.split_first() {
        None => t.is_empty(),
        Some(('*', rest)) => (0..=t.len()).any(|skip| glob_at(rest, &t[skip..])),
        Some(('?', rest)) => t.split_first().is_some_and(|(_, tr)| glob_at(rest, tr)),
        Some((c, rest)) => t
            .split_first()
            .is_some_and(|(tc, tr)| tc == c && glob_at(rest, tr)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn vault_in(dir: &Path) -> Vault {
        let root = dir.join("vault");
        fs::create_dir_all(&root).expect("mkdir");
        Vault::open(&root).expect("open")
    }

    #[test]
    fn kpignore_semantics() {
        let ig = KpIgnore::parse(
            "# comment\n\ndrafts/\n*.tmp.md\nnotes/secret.md\narchive/**/old-*.md\n",
        );
        // Directory pattern, any depth.
        assert!(ig.matches("drafts/a.md"));
        assert!(ig.matches("drafts/deep/b.md"));
        assert!(ig.matches("x/drafts/c.md"));
        assert!(!ig.matches("drafts.md"));
        // Extension glob at any depth (no slash in pattern).
        assert!(ig.matches("scratch.tmp.md"));
        assert!(ig.matches("a/b/scratch.tmp.md"));
        assert!(!ig.matches("scratch.md"));
        // Root-anchored exact path.
        assert!(ig.matches("notes/secret.md"));
        assert!(!ig.matches("other/notes/secret.md"));
        // ** spans directories.
        assert!(ig.matches("archive/2020/q1/old-report.md"));
        assert!(ig.matches("archive/old-x.md"));
        assert!(!ig.matches("archive/new-x.md"));
    }

    #[test]
    fn empty_and_comment_only_ignores_nothing() {
        let ig = KpIgnore::parse("\n# just a comment\n   \n");
        assert!(!ig.matches("a.md"));
        assert!(!ig.matches("deep/b.md"));
    }

    #[test]
    fn question_mark_matches_one_char() {
        let ig = KpIgnore::parse("note-?.md\n");
        assert!(ig.matches("note-1.md"));
        assert!(ig.matches("sub/note-x.md"));
        assert!(!ig.matches("note-10.md"));
    }

    #[test]
    fn walk_parses_notes_and_honors_kpignore() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = vault_in(dir.path());
        vault.write_atomic("keep.md", "# Keep\n").expect("write");
        vault
            .write_atomic("drafts/wip.md", "# WIP\n")
            .expect("write");
        vault
            .write_atomic("scratch.tmp.md", "# Tmp\n")
            .expect("write");
        vault
            .write_atomic(".kpignore", "drafts/\n*.tmp.md\n")
            .expect("write");

        let report = walk_vault(&vault).expect("walk");
        let paths: Vec<&str> = report.notes.iter().map(|n| n.rel_path.as_str()).collect();
        assert_eq!(paths, vec!["keep.md"]);
        let mut ignored = report.ignored.clone();
        ignored.sort();
        assert_eq!(ignored, vec!["drafts/wip.md", "scratch.tmp.md"]);
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn unparseable_notes_are_skipped_with_a_warning_not_fatal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = vault_in(dir.path());
        vault.write_atomic("good.md", "fine\n").expect("write");
        // Unterminated frontmatter fence: a parse error.
        vault
            .write_atomic("bad.md", "---\nkp_id: \"kp:1\"\nnever closed\n")
            .expect("write");
        let report = walk_vault(&vault).expect("walk");
        assert_eq!(report.notes.len(), 1);
        assert_eq!(report.notes[0].rel_path, "good.md");
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].0, "bad.md");
        assert!(report.skipped[0].1.contains("unterminated"));
    }

    #[test]
    fn missing_kpignore_walks_everything() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = vault_in(dir.path());
        vault.write_atomic("a.md", "a\n").expect("write");
        vault.write_atomic("sub/b.md", "b\n").expect("write");
        let report = walk_vault(&vault).expect("walk");
        assert_eq!(report.notes.len(), 2);
        assert!(report.ignored.is_empty());
    }
}
