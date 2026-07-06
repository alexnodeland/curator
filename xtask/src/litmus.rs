//! The grep litmus.
//!
//! This is a PUBLIC product repo: it must contain zero references to any
//! private reference-deployment — no LAN prefixes, no internal service
//! names, no host topology. The litmus scans the whole repository
//! (skipping `.git/`, `target/`, `.kp/`) for a banned pattern set and
//! exits nonzero on any hit. One pattern (a proprietary-editor name) is
//! additionally allowed inside `docs/` narrative text only — never in
//! code, config defaults, or contracts.
//!
//! The banned literals are assembled from halves at runtime so that this
//! source file — which the litmus also scans — never contains a banned
//! string itself.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Directories never scanned (VCS state, build output, local plane state).
const SKIP_DIRS: [&str; 3] = [".git", "target", ".kp"];

/// Path prefix (repo-relative, `/`-separated) where the docs-exempt
/// pattern is tolerated in narrative text.
const DOCS_PREFIX: &str = "docs/";

/// Patterns banned everywhere in the repo, docs included.
/// Assembled from halves — see module docs.
fn banned_everywhere() -> Vec<String> {
    let halves: [(&str, &str); 6] = [
        ("192.16", "8.68"), // private LAN prefix
        ("lite", "llm"),    // internal LLM gateway
        ("lang", "fuse"),   // internal tracing service
        ("supa", "base"),   // internal database service
        ("for", "gejo"),    // internal git forge
        ("nt", "fy"),       // internal notification service
    ];
    halves.iter().map(|(a, b)| format!("{a}{b}")).collect()
}

/// Patterns banned outside `docs/` (allowed in docs narrative only).
fn banned_outside_docs() -> Vec<String> {
    vec![format!("{}{}", "obsi", "dian")] // proprietary editor name
}

/// One litmus hit.
#[derive(Debug, PartialEq, Eq)]
pub struct Finding {
    /// Repo-relative path.
    pub path: PathBuf,
    /// 1-based line number.
    pub line: usize,
    /// The banned pattern that matched.
    pub pattern: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}: banned pattern {:?}",
            self.path.display(),
            self.line,
            self.pattern
        )
    }
}

/// Scan `root` and return every banned-pattern hit.
///
/// Matching is case-insensitive substring matching over UTF-8 text files;
/// non-UTF-8 files are treated as binary and skipped.
pub fn scan(root: &Path) -> io::Result<Vec<Finding>> {
    let everywhere = banned_everywhere();
    let outside_docs = banned_outside_docs();
    let mut findings = Vec::new();
    walk(root, root, &everywhere, &outside_docs, &mut findings)?;
    findings.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
    Ok(findings)
}

fn walk(
    root: &Path,
    dir: &Path,
    everywhere: &[String],
    outside_docs: &[String],
    findings: &mut Vec<Finding>,
) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if SKIP_DIRS.contains(&name.as_ref()) {
                continue;
            }
            walk(root, &path, everywhere, outside_docs, findings)?;
        } else if file_type.is_file() {
            scan_file(root, &path, everywhere, outside_docs, findings)?;
        }
        // Symlinks are skipped: the repo defines its own content.
    }
    Ok(())
}

fn scan_file(
    root: &Path,
    path: &Path,
    everywhere: &[String],
    outside_docs: &[String],
    findings: &mut Vec<Finding>,
) -> io::Result<()> {
    let Ok(content) = fs::read_to_string(path) else {
        return Ok(()); // non-UTF-8 → binary → skip
    };
    let rel: PathBuf = path.strip_prefix(root).unwrap_or(path).to_path_buf();
    let rel_slash = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    let in_docs = rel_slash.starts_with(DOCS_PREFIX);

    for (idx, line) in content.lines().enumerate() {
        let lower = line.to_lowercase();
        for pattern in everywhere {
            if lower.contains(pattern.as_str()) {
                findings.push(Finding {
                    path: rel.clone(),
                    line: idx + 1,
                    pattern: pattern.clone(),
                });
            }
        }
        if !in_docs {
            for pattern in outside_docs {
                if lower.contains(pattern.as_str()) {
                    findings.push(Finding {
                        path: rel.clone(),
                        line: idx + 1,
                        pattern: pattern.clone(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// CLI entrypoint: scan and report.
pub fn run(root_arg: Option<&str>) -> ExitCode {
    // Default root: the workspace root (parent of this crate's manifest dir).
    let default_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let root = root_arg.map_or(default_root, PathBuf::from);

    match scan(&root) {
        Ok(findings) if findings.is_empty() => {
            println!("litmus: clean ({} scanned)", root.display());
            ExitCode::SUCCESS
        }
        Ok(findings) => {
            eprintln!("litmus: {} banned-pattern hit(s):", findings.len());
            for f in &findings {
                eprintln!("  {f}");
            }
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!("litmus: scan failed: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch repo layout in a unique temp dir; removed on drop.
    struct TempRepo {
        root: PathBuf,
    }

    impl TempRepo {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "curator-litmus-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            fs::create_dir_all(&root).expect("create temp repo");
            Self { root }
        }

        fn write(&self, rel: &str, content: &str) {
            let path = self.root.join(rel);
            fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            fs::write(path, content).expect("write");
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn clean_tree_produces_no_findings() {
        let repo = TempRepo::new("clean");
        repo.write("crates/curator-core/src/lib.rs", "pub fn nothing() {}\n");
        repo.write("contracts/kp-note/v1.md", "# kp-note/v1\n");
        assert_eq!(scan(&repo.root).expect("scan").len(), 0);
    }

    #[test]
    fn catches_a_planted_string_in_a_tempdir() {
        let repo = TempRepo::new("planted");
        let planted = format!("db = \"{}{}\"", "supa", "base");
        repo.write("crates/curator-core/src/lib.rs", &planted);
        let findings = scan(&repo.root).expect("scan");
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].path,
            PathBuf::from("crates/curator-core/src/lib.rs")
        );
        assert_eq!(findings[0].line, 1);
        assert_eq!(findings[0].pattern, format!("{}{}", "supa", "base"));
    }

    #[test]
    fn matching_is_case_insensitive() {
        let repo = TempRepo::new("case");
        repo.write("kp.toml", &format!("# {}{}", "ForGe", "jO"));
        assert_eq!(scan(&repo.root).expect("scan").len(), 1);
    }

    #[test]
    fn every_banned_pattern_is_caught() {
        let repo = TempRepo::new("all");
        let all: Vec<String> = banned_everywhere();
        repo.write("contracts/x.md", &all.join("\n"));
        let findings = scan(&repo.root).expect("scan");
        assert_eq!(findings.len(), all.len());
    }

    #[test]
    fn docs_exempt_pattern_is_allowed_in_docs_only() {
        let repo = TempRepo::new("docs");
        let word = format!("{}{}", "Obsi", "dian");
        repo.write("docs/design/notes.md", &format!("{word} is one viewer.\n"));
        repo.write("crates/curator-core/src/lib.rs", &format!("// {word}\n"));
        let findings = scan(&repo.root).expect("scan");
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].path,
            PathBuf::from("crates/curator-core/src/lib.rs")
        );
    }

    #[test]
    fn hard_banned_patterns_are_banned_even_in_docs() {
        let repo = TempRepo::new("docs-hard");
        repo.write(
            "docs/design/old.md",
            &format!("host {}{}\n", "192.16", "8.68"),
        );
        assert_eq!(scan(&repo.root).expect("scan").len(), 1);
    }

    #[test]
    fn skip_dirs_are_not_scanned() {
        let repo = TempRepo::new("skip");
        let planted = format!("{}{}", "lang", "fuse");
        repo.write("target/debug/build.log", &planted);
        repo.write(".git/config", &planted);
        repo.write(".kp/cursors/state", &planted);
        assert_eq!(scan(&repo.root).expect("scan").len(), 0);
    }

    #[test]
    fn the_real_repo_is_clean() {
        // The litmus's own definition of the workspace root.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root");
        let findings = scan(root).expect("scan");
        assert!(
            findings.is_empty(),
            "banned patterns present in the repo:\n{}",
            findings
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
