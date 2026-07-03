//! The vault: a plain markdown directory, addressed ONLY by vault-relative
//! paths.
//!
//! The vault is canonical, human-owned content — the one thing the plane
//! must never damage or leak past. Every filesystem touch goes through
//! [`Vault::resolve`], which enforces three hard guarantees:
//!
//! 1. **no absolute paths** — a note address is always vault-relative;
//! 2. **no traversal** — any `..` component is rejected outright, even
//!    ones that would normalize back inside the root;
//! 3. **no symlink escape** — a path whose resolution (following symlinks
//!    in any component) lands outside the canonicalized root is rejected,
//!    including dangling symlinks.
//!
//! Writes are atomic (same-directory temp file + rename): a crash mid-write
//! never leaves a half-written note.

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::note::{Note, NoteError};

/// Errors from vault operations.
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    /// The vault root does not exist or is not a directory.
    #[error("vault root is not a directory: {0}")]
    NotADirectory(PathBuf),
    /// An underlying filesystem error.
    #[error("vault I/O on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The relative path is empty.
    #[error("empty vault-relative path")]
    EmptyPath,
    /// The path is absolute — note addresses are always vault-relative.
    #[error("absolute path rejected: {0}")]
    AbsolutePath(String),
    /// The path contains a `..` component.
    #[error("path traversal rejected: {0}")]
    Traversal(String),
    /// The path resolves (via symlinks) outside the vault root.
    #[error("path escapes the vault via symlink: {rel} -> {resolved}")]
    SymlinkEscape { rel: String, resolved: PathBuf },
    /// The note file failed to parse.
    #[error(transparent)]
    Note(#[from] NoteError),
}

/// An open vault root.
#[derive(Debug, Clone)]
pub struct Vault {
    /// Canonicalized root — symlink-free by construction, so `starts_with`
    /// containment checks against it are sound.
    root: PathBuf,
}

impl Vault {
    /// Open a vault at `root`. The root must exist and be a directory.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, VaultError> {
        let root = root.as_ref();
        let canon =
            fs::canonicalize(root).map_err(|_| VaultError::NotADirectory(root.to_owned()))?;
        if !canon.is_dir() {
            return Err(VaultError::NotADirectory(root.to_owned()));
        }
        Ok(Self { root: canon })
    }

    /// The canonicalized vault root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a vault-relative path to an absolute one, enforcing the
    /// module-level safety guarantees. The target itself need not exist
    /// (writes create files), but every EXISTING prefix of it must resolve
    /// inside the root.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf, VaultError> {
        if rel.is_empty() {
            return Err(VaultError::EmptyPath);
        }
        let rel_path = Path::new(rel);
        if rel_path.is_absolute() {
            return Err(VaultError::AbsolutePath(rel.to_owned()));
        }
        for component in rel_path.components() {
            match component {
                // Rejected even when it would normalize back inside the
                // root ("a/../b"): lexical normalization and symlink
                // resolution do not commute, so `..` is never safe to
                // reason about lexically. Producers write plain paths.
                Component::ParentDir => return Err(VaultError::Traversal(rel.to_owned())),
                // Also treat Windows-style prefixes/roots as absolute.
                Component::Prefix(_) | Component::RootDir => {
                    return Err(VaultError::AbsolutePath(rel.to_owned()));
                }
                Component::CurDir | Component::Normal(_) => {}
            }
        }
        let joined = self.root.join(rel_path);

        // Symlink-escape check: canonicalize the deepest existing ancestor
        // (following symlinks) and require it inside the root. A dangling
        // symlink anywhere on the path fails canonicalization while
        // symlink_metadata still sees it — rejected conservatively.
        let mut probe: &Path = &joined;
        loop {
            if fs::symlink_metadata(probe).is_ok() {
                let resolved = fs::canonicalize(probe).map_err(|_| VaultError::SymlinkEscape {
                    rel: rel.to_owned(),
                    resolved: probe.to_owned(),
                })?;
                if !resolved.starts_with(&self.root) {
                    return Err(VaultError::SymlinkEscape {
                        rel: rel.to_owned(),
                        resolved,
                    });
                }
                break;
            }
            match probe.parent() {
                Some(parent) if parent.starts_with(&self.root) => probe = parent,
                // Reached the root itself (already canonical) or above.
                _ => break,
            }
        }
        Ok(joined)
    }

    /// Read a file's content by vault-relative path.
    pub fn read(&self, rel: &str) -> Result<String, VaultError> {
        let path = self.resolve(rel)?;
        fs::read_to_string(&path).map_err(|source| VaultError::Io { path, source })
    }

    /// Read and parse a note by vault-relative path.
    pub fn read_note(&self, rel: &str) -> Result<Note, VaultError> {
        let content = self.read(rel)?;
        Ok(Note::parse(rel, &content)?)
    }

    /// Atomically write a file by vault-relative path: write to a
    /// same-directory temp file, then rename over the target. Parent
    /// directories are created as needed (inside the root by construction).
    pub fn write_atomic(&self, rel: &str, content: &str) -> Result<(), VaultError> {
        let path = self.resolve(rel)?;
        let parent = path.parent().unwrap_or(&self.root).to_owned();
        fs::create_dir_all(&parent).map_err(|source| VaultError::Io {
            path: parent.clone(),
            source,
        })?;
        // Re-check AFTER create_dir_all: mkdir may have materialized
        // directories under a previously-dangling symlink chain.
        let path = self.resolve(rel)?;
        let file_name = path
            .file_name()
            .ok_or(VaultError::EmptyPath)?
            .to_string_lossy()
            .into_owned();
        let tmp = parent.join(format!(".{file_name}.kp-tmp-{}", std::process::id()));
        fs::write(&tmp, content).map_err(|source| VaultError::Io {
            path: tmp.clone(),
            source,
        })?;
        fs::rename(&tmp, &path).map_err(|source| {
            let _ = fs::remove_file(&tmp);
            VaultError::Io { path, source }
        })
    }

    /// Vault-relative paths (forward slashes, sorted) of every `.md` note.
    ///
    /// Dot-directories (`.kp`, `.curio`, `.git`, ...) are skipped: derived
    /// state and other tools' machinery are not notes. Symlinked
    /// directories are not followed — anything reachable only through a
    /// symlink is either duplicated (in-vault target) or out of bounds.
    pub fn note_paths(&self) -> Result<Vec<String>, VaultError> {
        let mut out = Vec::new();
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let entries = fs::read_dir(&dir).map_err(|source| VaultError::Io {
                path: dir.clone(),
                source,
            })?;
            for entry in entries {
                let entry = entry.map_err(|source| VaultError::Io {
                    path: dir.clone(),
                    source,
                })?;
                let name = entry.file_name();
                if name.to_string_lossy().starts_with('.') {
                    continue;
                }
                let path = entry.path();
                let meta = fs::symlink_metadata(&path).map_err(|source| VaultError::Io {
                    path: path.clone(),
                    source,
                })?;
                if meta.is_dir() {
                    stack.push(path);
                } else if meta.is_file() && path.extension().is_some_and(|e| e == "md") {
                    let rel = path
                        .strip_prefix(&self.root)
                        .expect("walk stays under root")
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/");
                    out.push(rel);
                }
            }
        }
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault_in(dir: &Path) -> Vault {
        let root = dir.join("vault");
        fs::create_dir_all(&root).expect("mkdir");
        Vault::open(&root).expect("open")
    }

    #[test]
    fn open_rejects_missing_or_file_roots() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(matches!(
            Vault::open(dir.path().join("nope")).unwrap_err(),
            VaultError::NotADirectory(_)
        ));
        let f = dir.path().join("file");
        fs::write(&f, "x").expect("write");
        assert!(matches!(
            Vault::open(&f).unwrap_err(),
            VaultError::NotADirectory(_)
        ));
    }

    #[test]
    fn resolves_plain_relative_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = vault_in(dir.path());
        let p = vault.resolve("notes/a.md").expect("resolves");
        assert!(p.starts_with(vault.root()));
        assert!(p.ends_with("notes/a.md"));
        // `./` is harmless.
        vault.resolve("./notes/a.md").expect("curdir ok");
    }

    #[test]
    fn rejects_empty_and_absolute_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = vault_in(dir.path());
        assert!(matches!(
            vault.resolve("").unwrap_err(),
            VaultError::EmptyPath
        ));
        assert!(matches!(
            vault.resolve("/etc/passwd").unwrap_err(),
            VaultError::AbsolutePath(_)
        ));
        // Absolute path INTO the vault is still rejected: addresses are
        // vault-relative, full stop.
        let inside = vault.root().join("a.md");
        assert!(matches!(
            vault.resolve(inside.to_str().expect("utf8")).unwrap_err(),
            VaultError::AbsolutePath(_)
        ));
    }

    #[test]
    fn rejects_every_traversal_shape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = vault_in(dir.path());
        for rel in [
            "..",
            "../x.md",
            "a/../x.md",
            "a/b/../../../x.md",
            "a/..",
            "./../x.md",
        ] {
            assert!(
                matches!(vault.resolve(rel).unwrap_err(), VaultError::Traversal(_)),
                "{rel} must be rejected as traversal"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_dir_escape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = vault_in(dir.path());
        let outside = dir.path().join("outside");
        fs::create_dir_all(&outside).expect("mkdir");
        fs::write(outside.join("secret.md"), "s").expect("write");
        std::os::unix::fs::symlink(&outside, vault.root().join("esc")).expect("symlink");
        // The relative path LOOKS clean — the escape is in the symlink.
        let err = vault.resolve("esc/secret.md").unwrap_err();
        assert!(
            matches!(err, VaultError::SymlinkEscape { .. }),
            "got {err:?}"
        );
        assert!(vault.read("esc/secret.md").is_err());
        assert!(vault.write_atomic("esc/new.md", "x").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_file_escape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = vault_in(dir.path());
        let secret = dir.path().join("secret.md");
        fs::write(&secret, "s").expect("write");
        std::os::unix::fs::symlink(&secret, vault.root().join("leak.md")).expect("symlink");
        assert!(matches!(
            vault.resolve("leak.md").unwrap_err(),
            VaultError::SymlinkEscape { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_dangling_symlink() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = vault_in(dir.path());
        std::os::unix::fs::symlink(dir.path().join("gone"), vault.root().join("dangle.md"))
            .expect("symlink");
        assert!(matches!(
            vault.resolve("dangle.md").unwrap_err(),
            VaultError::SymlinkEscape { .. }
        ));
        // And writing THROUGH a dangling directory symlink is rejected too.
        std::os::unix::fs::symlink(dir.path().join("gone-dir"), vault.root().join("ghost"))
            .expect("symlink");
        assert!(vault.write_atomic("ghost/new.md", "x").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_within_the_vault_are_fine() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = vault_in(dir.path());
        let real = vault.root().join("real");
        fs::create_dir_all(&real).expect("mkdir");
        fs::write(real.join("n.md"), "hello").expect("write");
        std::os::unix::fs::symlink(&real, vault.root().join("alias")).expect("symlink");
        vault
            .resolve("alias/n.md")
            .expect("in-vault symlink resolves");
        assert_eq!(vault.read("alias/n.md").expect("reads"), "hello");
    }

    #[cfg(unix)]
    #[test]
    fn vault_root_may_itself_be_a_symlink() {
        // Common on macOS (/tmp -> /private/tmp) — the canonicalized root
        // is what containment is checked against.
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("real-vault");
        fs::create_dir_all(&real).expect("mkdir");
        std::os::unix::fs::symlink(&real, dir.path().join("link-vault")).expect("symlink");
        let vault = Vault::open(dir.path().join("link-vault")).expect("open via symlink");
        vault.write_atomic("a.md", "x").expect("write");
        assert_eq!(vault.read("a.md").expect("read"), "x");
    }

    #[test]
    fn write_atomic_round_trips_and_creates_parents() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = vault_in(dir.path());
        vault
            .write_atomic("deep/nested/n.md", "content\n")
            .expect("write");
        assert_eq!(vault.read("deep/nested/n.md").expect("read"), "content\n");
        // Overwrite is atomic replace, not append.
        vault
            .write_atomic("deep/nested/n.md", "v2\n")
            .expect("rewrite");
        assert_eq!(vault.read("deep/nested/n.md").expect("read"), "v2\n");
        // No temp litter left behind.
        let entries: Vec<_> = fs::read_dir(vault.root().join("deep/nested"))
            .expect("readdir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["n.md"]);
    }

    #[test]
    fn read_note_parses_and_keys_by_rel_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = vault_in(dir.path());
        vault.write_atomic("plain.md", "# body\n").expect("write");
        let note = vault.read_note("plain.md").expect("parses");
        assert_eq!(note.kp_id().to_string(), "path:plain.md");
        assert_eq!(note.body, "# body\n");
    }

    #[test]
    fn note_paths_walks_md_files_and_skips_dot_dirs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = vault_in(dir.path());
        vault.write_atomic("a.md", "a").expect("write");
        vault.write_atomic("sub/b.md", "b").expect("write");
        vault.write_atomic("sub/deep/c.md", "c").expect("write");
        // Non-notes and dot-dirs must not appear.
        fs::write(vault.root().join("data.json"), "{}").expect("write");
        fs::create_dir_all(vault.root().join(".kp/proposals")).expect("mkdir");
        fs::write(vault.root().join(".kp/proposals/p.md"), "x").expect("write");
        fs::create_dir_all(vault.root().join(".curio")).expect("mkdir");
        fs::write(vault.root().join(".curio/manifest.json"), "{}").expect("write");
        assert_eq!(
            vault.note_paths().expect("walk"),
            vec!["a.md", "sub/b.md", "sub/deep/c.md"]
        );
    }
}
