//! `proposals/v1` — the ONLY write path for agents
//! (contract: `contracts/proposals/v1.md`).
//!
//! Local-first and forge-free: the validator works with no git remote at
//! all. Layout: `<vault>/.kp/proposals/<ULID>/` containing `proposal.json`
//! (this type) + `changes.patch` (unified diff against the vault tree).
//!
//! [`create_proposal`] is the shared write primitive: both `kp_propose`
//! (the MCP surface's single write verb) and the librarian's digest loop
//! ride it. It never touches the target files — it validates the intent,
//! renders the diff, and records the proposal for human review/`kp apply`.

use serde::{Deserialize, Serialize};

use crate::managed::{CURIO_FRONTMATTER_SCHEMA, managed_block};
use crate::note::{Frontmatter, Note};
use crate::time::now_rfc3339_utc;
use crate::vault::{Vault, VaultError};

/// The `schema` value this crate implements.
pub const PROPOSALS_SCHEMA: &str = "proposals/v1";

/// `proposal.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    /// Always `proposals/v1` for this version.
    pub schema: String,
    /// ULID — sortable proposal id, also the directory name.
    pub id: String,
    /// RFC 3339 UTC creation timestamp.
    pub created: String,
    /// `kp-librarian` or an agent-supplied name.
    pub author: String,
    pub title: String,
    pub rationale: String,
    pub status: ProposalStatus,
    /// Vault-relative paths touched by `changes.patch`.
    pub files: Vec<String>,
}

/// Proposal lifecycle. Stamped by `kp apply` / `kp reject` — never edited
/// by agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalStatus {
    Open,
    Applied,
    Rejected,
}

/// One proposed file: a vault-relative path plus its FULL new content
/// (create or whole-file replace — the primitive renders the diff).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalFile {
    /// Vault-relative path, forward slashes.
    pub path: String,
    /// The complete intended content of the file.
    pub content: String,
}

/// Errors from [`create_proposal`]. Every rejection here is a contract
/// hard-reject — there is no override flag.
#[derive(Debug, thiserror::Error)]
pub enum ProposalWriteError {
    /// A proposal must touch at least one file.
    #[error("proposal has no files")]
    NoFiles,
    /// The same path appears twice in one proposal.
    #[error("duplicate path in proposal: {0}")]
    DuplicatePath(String),
    /// `.curio/**` is producer-owned state (contract hard-reject 1).
    #[error("path under .curio/ is producer-owned and never writable: {0}")]
    CurioPath(String),
    /// Dot-directories (`.kp`, `.git`, ...) are machinery, not notes.
    /// Stricter than the contract minimum, deliberately: nothing an agent
    /// proposes belongs in hidden state.
    #[error("path under a dot-directory rejected: {0}")]
    DotPath(String),
    /// Path safety (absolute, traversal, symlink escape) or I/O failure,
    /// as enforced by [`Vault::resolve`] (contract hard-reject 3).
    #[error(transparent)]
    Vault(#[from] VaultError),
    /// The edit reaches inside Curio's machine surface — the managed
    /// region or Curio-owned frontmatter keys (contract hard-reject 2).
    #[error("curio-owned content edit rejected in {path}: {reason}")]
    CurioOwnedEdit { path: String, reason: String },
    /// The proposed content is byte-identical to the existing file.
    #[error("proposed content is identical to the existing file: {0}")]
    NoChange(String),
    /// `proposal.json` failed to serialize (never expected).
    #[error("proposal serialization: {0}")]
    Json(#[from] serde_json::Error),
}

/// Create a `proposals/v1` proposal in `<vault>/<proposals_dir>/<ULID>/`.
///
/// Validates every target path (vault-relative, no `.curio/**`, no
/// dot-directories, no Curio managed-region or machine-key edits),
/// renders `changes.patch` as a unified diff against the current vault
/// tree, and writes `proposal.json` with `status: open`. Target files
/// themselves are NOT written — that is `kp apply`'s job, after human
/// review.
pub fn create_proposal(
    vault: &Vault,
    proposals_dir: &str,
    author: &str,
    title: &str,
    rationale: &str,
    files: &[ProposalFile],
) -> Result<Proposal, ProposalWriteError> {
    if files.is_empty() {
        return Err(ProposalWriteError::NoFiles);
    }
    let mut seen: Vec<&str> = Vec::with_capacity(files.len());
    let mut patch = String::new();
    for file in files {
        if seen.contains(&file.path.as_str()) {
            return Err(ProposalWriteError::DuplicatePath(file.path.clone()));
        }
        seen.push(&file.path);
        // Vault safety first (absolute/traversal/symlink shapes get their
        // precise error), then the plane's own dot-dir policy.
        let resolved = vault.resolve(&file.path)?;
        validate_target_path(&file.path)?;
        let old = if resolved.exists() {
            Some(vault.read(&file.path)?)
        } else {
            None
        };
        if let Some(old) = &old {
            guard_curio_surface(&file.path, old, &file.content)?;
            if *old == file.content {
                return Err(ProposalWriteError::NoChange(file.path.clone()));
            }
        }
        patch.push_str(&render_unified_diff(
            &file.path,
            old.as_deref(),
            &file.content,
        ));
    }

    let proposal = Proposal {
        schema: PROPOSALS_SCHEMA.to_owned(),
        id: ulid::Ulid::new().to_string(),
        created: now_rfc3339_utc(),
        author: author.to_owned(),
        title: title.to_owned(),
        rationale: rationale.to_owned(),
        status: ProposalStatus::Open,
        files: files.iter().map(|f| f.path.clone()).collect(),
    };

    let dir = format!("{}/{}", proposals_dir.trim_end_matches('/'), proposal.id);
    let mut json = serde_json::to_string_pretty(&proposal)?;
    json.push('\n');
    vault.write_atomic(&format!("{dir}/proposal.json"), &json)?;
    vault.write_atomic(&format!("{dir}/changes.patch"), &patch)?;
    Ok(proposal)
}

/// Reject `.curio/**` (contract) and any other dot-directory-rooted path
/// (stricter, see [`ProposalWriteError::DotPath`]). [`Vault::resolve`]
/// covers absolute/traversal/symlink shapes separately.
fn validate_target_path(path: &str) -> Result<(), ProposalWriteError> {
    let first = path.split('/').next().unwrap_or(path);
    if first == ".curio" {
        return Err(ProposalWriteError::CurioPath(path.to_owned()));
    }
    if first.starts_with('.') {
        return Err(ProposalWriteError::DotPath(path.to_owned()));
    }
    Ok(())
}

/// Contract hard-reject 2: when the existing file is Curio's (managed
/// markers in the body, or `schema: curio.frontmatter.v1` frontmatter),
/// the new content must preserve the managed region byte-exact and keep
/// every existing frontmatter key with its existing value — enrichment
/// may only ADD keys and companion content.
fn guard_curio_surface(path: &str, old: &str, new: &str) -> Result<(), ProposalWriteError> {
    let Ok(old_note) = Note::parse(path, old) else {
        return Ok(()); // unparseable old file: nothing Curio-shaped to protect
    };
    let old_managed = managed_block(&old_note.body);
    let is_curio = old_managed.is_some()
        || matches!(
            &old_note.frontmatter,
            Frontmatter::Foreign(yaml) if foreign_schema(yaml) == Some(CURIO_FRONTMATTER_SCHEMA)
        );
    if !is_curio {
        return Ok(());
    }
    let new_note = Note::parse(path, new).map_err(|e| ProposalWriteError::CurioOwnedEdit {
        path: path.to_owned(),
        reason: format!("replacement does not parse as a note: {e}"),
    })?;
    if let Some(old_block) = old_managed
        && managed_block(&new_note.body) != Some(old_block)
    {
        return Err(ProposalWriteError::CurioOwnedEdit {
            path: path.to_owned(),
            reason: "the curio:managed region must be preserved byte-exact".to_owned(),
        });
    }
    // Frontmatter: every old key must survive with an equal value.
    let old_map = frontmatter_map(&old_note.frontmatter);
    let new_map = frontmatter_map(&new_note.frontmatter);
    for (key, old_value) in &old_map {
        if new_map.get(key) != Some(old_value) {
            return Err(ProposalWriteError::CurioOwnedEdit {
                path: path.to_owned(),
                reason: format!(
                    "existing frontmatter key {key:?} was modified or removed \
                     (enrichment may only add keys)"
                ),
            });
        }
    }
    Ok(())
}

fn foreign_schema(yaml: &str) -> Option<&str> {
    // Leak-free borrow trick is not worth it; parse loosely each time.
    // Guard paths run once per proposed file.
    let value: serde_yaml::Value = serde_yaml::from_str(yaml).ok()?;
    match value.get("schema") {
        Some(serde_yaml::Value::String(s)) if s == CURIO_FRONTMATTER_SCHEMA => {
            Some(CURIO_FRONTMATTER_SCHEMA)
        }
        _ => None,
    }
}

fn frontmatter_map(fm: &Frontmatter) -> serde_yaml::Mapping {
    let yaml = match fm {
        Frontmatter::None => return serde_yaml::Mapping::new(),
        Frontmatter::Foreign(yaml) => yaml.clone(),
        Frontmatter::Kp(fm) => {
            serde_yaml::to_string(fm).expect("kp-note/v1 block always serializes")
        }
    };
    serde_yaml::from_str(&yaml).unwrap_or_default()
}

/// One file's unified diff: `/dev/null` → `b/<path>` for creations,
/// `a/<path>` → `b/<path>` for whole-file replacements.
fn render_unified_diff(path: &str, old: Option<&str>, new: &str) -> String {
    let old_text = old.unwrap_or("");
    let old_header = if old.is_some() {
        format!("a/{path}")
    } else {
        "/dev/null".to_owned()
    };
    let diff = similar::TextDiff::from_lines(old_text, new);
    diff.unified_diff()
        .context_radius(3)
        .header(&old_header, &format!("b/{path}"))
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managed::{MANAGED_BEGIN, MANAGED_END};

    #[test]
    fn round_trips_the_contract_example() {
        let raw = r#"{
            "schema": "proposals/v1",
            "id": "01J1PZ2M3N4P5Q6R7S8T9V0W1X",
            "created": "2026-07-03T09:15:00Z",
            "author": "kp-librarian",
            "title": "Daily digest 2026-07-03",
            "rationale": "12 new notes since last digest matched the now.md anchor.",
            "status": "open",
            "files": ["digests/2026-07-03.md"]
        }"#;
        let p: Proposal = serde_json::from_str(raw).expect("should parse");
        assert_eq!(p.schema, PROPOSALS_SCHEMA);
        assert_eq!(p.status, ProposalStatus::Open);
        let back = serde_json::to_string(&p).expect("should serialize");
        let p2: Proposal = serde_json::from_str(&back).expect("should re-parse");
        assert_eq!(p, p2);
    }

    #[test]
    fn status_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&ProposalStatus::Applied).expect("serialize"),
            "\"applied\""
        );
        assert_eq!(
            serde_json::to_string(&ProposalStatus::Rejected).expect("serialize"),
            "\"rejected\""
        );
    }

    fn tmp_vault(dir: &std::path::Path) -> Vault {
        let root = dir.join("vault");
        std::fs::create_dir_all(&root).expect("mkdir");
        Vault::open(&root).expect("open")
    }

    fn file(path: &str, content: &str) -> ProposalFile {
        ProposalFile {
            path: path.to_owned(),
            content: content.to_owned(),
        }
    }

    #[test]
    fn creates_the_contract_layout_for_a_new_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        let p = create_proposal(
            &vault,
            ".kp/proposals",
            "test-agent",
            "Add a note",
            "because",
            &[file("notes/idea.md", "# Idea\n\nbody\n")],
        )
        .expect("creates");

        assert_eq!(p.schema, PROPOSALS_SCHEMA);
        assert_eq!(p.status, ProposalStatus::Open);
        assert_eq!(p.files, vec!["notes/idea.md"]);
        assert_eq!(p.id.len(), 26, "ULID is 26 Crockford chars");
        assert!(
            p.id.chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        );
        assert_eq!(p.created.len(), 20);
        assert!(p.created.ends_with('Z'));

        let base = format!(".kp/proposals/{}", p.id);
        let json = vault.read(&format!("{base}/proposal.json")).expect("json");
        let back: Proposal = serde_json::from_str(&json).expect("parses");
        assert_eq!(back, p);
        let patch = vault
            .read(&format!("{base}/changes.patch"))
            .expect("patch exists");
        assert!(patch.contains("--- /dev/null"));
        assert!(patch.contains("+++ b/notes/idea.md"));
        assert!(patch.contains("+# Idea"));

        // The primitive proposes; it never writes the target.
        assert!(vault.read("notes/idea.md").is_err());
    }

    #[test]
    fn modification_diffs_against_the_existing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        vault
            .write_atomic("n.md", "line one\nline two\n")
            .expect("seed");
        let p = create_proposal(
            &vault,
            ".kp/proposals",
            "a",
            "t",
            "r",
            &[file("n.md", "line one\nline 2\n")],
        )
        .expect("creates");
        let patch = vault
            .read(&format!(".kp/proposals/{}/changes.patch", p.id))
            .expect("patch");
        assert!(patch.contains("--- a/n.md"));
        assert!(patch.contains("+++ b/n.md"));
        assert!(patch.contains("-line two"));
        assert!(patch.contains("+line 2"));
        // The vault file is untouched.
        assert_eq!(vault.read("n.md").expect("read"), "line one\nline two\n");
    }

    #[test]
    fn distinct_proposals_get_distinct_sortable_ids() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        let mk = || {
            create_proposal(
                &vault,
                ".kp/proposals",
                "a",
                "t",
                "r",
                &[file("x.md", "content\n")],
            )
            .expect("creates")
        };
        let (a, b) = (mk(), mk());
        assert_ne!(a.id, b.id);
        assert!(a.id <= b.id, "ULIDs sort by creation time");
    }

    #[test]
    fn hard_rejects_bad_shapes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        let create =
            |files: &[ProposalFile]| create_proposal(&vault, ".kp/proposals", "a", "t", "r", files);

        assert!(matches!(create(&[]), Err(ProposalWriteError::NoFiles)));
        assert!(matches!(
            create(&[file("a.md", "x"), file("a.md", "y")]),
            Err(ProposalWriteError::DuplicatePath(_))
        ));
        assert!(matches!(
            create(&[file(".curio/manifest.json", "{}")]),
            Err(ProposalWriteError::CurioPath(_))
        ));
        assert!(matches!(
            create(&[file(".kp/proposals/evil/proposal.json", "{}")]),
            Err(ProposalWriteError::DotPath(_))
        ));
        assert!(matches!(
            create(&[file("../escape.md", "x")]),
            Err(ProposalWriteError::Vault(VaultError::Traversal(_)))
        ));
        assert!(matches!(
            create(&[file("/abs.md", "x")]),
            Err(ProposalWriteError::Vault(VaultError::AbsolutePath(_)))
        ));

        // Nothing was written for any rejection.
        assert_eq!(std::fs::read_dir(vault.root()).expect("readdir").count(), 0);
    }

    #[test]
    fn rejects_identical_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        vault.write_atomic("same.md", "unchanged\n").expect("seed");
        assert!(matches!(
            create_proposal(
                &vault,
                ".kp/proposals",
                "a",
                "t",
                "r",
                &[file("same.md", "unchanged\n")],
            ),
            Err(ProposalWriteError::NoChange(_))
        ));
    }

    fn curio_note(companion: &str) -> String {
        format!(
            "---\nschema: curio.frontmatter.v1\ncurio_id: \"0197\"\ntitle: T\n---\n\
             {MANAGED_BEGIN}\nmachine text\n{MANAGED_END}\n{companion}"
        )
    }

    #[test]
    fn curio_companion_appends_are_allowed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        vault
            .write_atomic("curio/a.md", &curio_note(""))
            .expect("seed");
        create_proposal(
            &vault,
            ".kp/proposals",
            "a",
            "t",
            "r",
            &[file("curio/a.md", &curio_note("\n## My notes\n\nbelow\n"))],
        )
        .expect("companion enrichment below the region is legal");
    }

    #[test]
    fn curio_managed_region_edits_are_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        vault
            .write_atomic("curio/a.md", &curio_note(""))
            .expect("seed");
        let tampered = curio_note("").replace("machine text", "edited");
        let err = create_proposal(
            &vault,
            ".kp/proposals",
            "a",
            "t",
            "r",
            &[file("curio/a.md", &tampered)],
        )
        .unwrap_err();
        assert!(
            matches!(err, ProposalWriteError::CurioOwnedEdit { ref reason, .. }
                if reason.contains("managed region")),
            "got {err}"
        );
        // Dropping the region entirely is equally rejected.
        let err = create_proposal(
            &vault,
            ".kp/proposals",
            "a",
            "t",
            "r",
            &[file("curio/a.md", "no region at all\n")],
        )
        .unwrap_err();
        assert!(matches!(err, ProposalWriteError::CurioOwnedEdit { .. }));
    }

    #[test]
    fn curio_machine_frontmatter_keys_are_immutable_but_extensible() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        vault
            .write_atomic("curio/a.md", &curio_note(""))
            .expect("seed");

        // Modifying a machine key is rejected...
        let retitled = curio_note("").replace("title: T", "title: Renamed");
        let err = create_proposal(
            &vault,
            ".kp/proposals",
            "a",
            "t",
            "r",
            &[file("curio/a.md", &retitled)],
        )
        .unwrap_err();
        assert!(
            matches!(err, ProposalWriteError::CurioOwnedEdit { ref reason, .. }
                if reason.contains("title")),
            "got {err}"
        );

        // ...adding an enrichment key outside the machine set is fine.
        let enriched = curio_note("").replace("title: T\n", "title: T\nkp_rating: 5\n");
        create_proposal(
            &vault,
            ".kp/proposals",
            "a",
            "t",
            "r",
            &[file("curio/a.md", &enriched)],
        )
        .expect("additive frontmatter enrichment is legal");
    }

    #[test]
    fn plain_notes_have_no_curio_guard() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        vault
            .write_atomic("plain.md", "---\ntitle: mine\n---\nold\n")
            .expect("seed");
        create_proposal(
            &vault,
            ".kp/proposals",
            "a",
            "t",
            "r",
            &[file("plain.md", "totally rewritten\n")],
        )
        .expect("non-curio notes may be rewritten freely");
    }
}
