//! The apply/review side of `proposals/v1`.
//!
//! curator-core's `create_proposal` writes proposals; this module disposes of
//! them. [`apply_proposal`] runs the deterministic validator and either
//! writes the files + stamps `applied`, or stamps `rejected` and reports
//! why. There is no override flag; every hard-reject is final for that
//! proposal (fix the problem, propose again).
//!
//! The validator (contract `contracts/proposals/v1.md`):
//!
//! 1. any path under `.curio/**` (or any dot-directory) — rejected;
//! 2. any hunk editing Curio machine frontmatter keys or content inside a
//!    `curio:managed` region — rejected; ownership is detected by shape
//!    (managed markers / `curio.frontmatter.v1`) AND by the
//!    `.curio/manifest.json` ownership oracle (curator-ingest);
//! 3. any path escaping the vault (absolute, `..`, symlink) — rejected;
//! 4. patches that do not apply cleanly (strict, zero fuzz) — rejected;
//! 5. new notes whose identity — the explicit `kp_id`, or the implicit
//!    `path:<relpath>` identity of a plain note (kp-note/v1: identity is
//!    never absent) — duplicates an existing identity — rejected.

use std::collections::BTreeSet;

use curator_core::note::{Frontmatter, Note};
use curator_core::{
    Proposal, ProposalStatus, ProposalStoreError, Vault, VaultError, enforce_curio_preservation,
    is_curio_shaped, load_proposal, store_proposal_status, validate_target_path,
};
use curator_ingest::CurioManifest;

use crate::patch::{FilePatch, apply_file_patch, parse_patch};
use crate::uuid7::is_uuid7;

/// Errors from [`apply_proposal`].
#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    /// The proposal is not `open` — transitions are one-way.
    #[error("proposal {id} is already {status}")]
    NotOpen { id: String, status: String },
    /// The validator refused; the proposal has been stamped `rejected`.
    #[error("proposal {id} rejected: {reason}")]
    Rejected { id: String, reason: String },
    /// Reading/updating the stored proposal failed (environment, not
    /// validation — nothing was stamped).
    #[error(transparent)]
    Store(#[from] ProposalStoreError),
    /// Vault I/O failed mid-write (environment, not validation). Files
    /// written before the failure are reverted (best-effort), and the
    /// proposal stays `open` so a retry re-validates against the
    /// unchanged tree.
    #[error(transparent)]
    Vault(#[from] VaultError),
}

/// What one successful apply did.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ApplyReport {
    pub id: String,
    pub title: String,
    /// Vault-relative paths written.
    pub files_written: Vec<String>,
    pub status: String,
}

/// Validate and apply one stored proposal: on success every target file
/// is written (atomically, one by one) and `status: applied` is stamped;
/// on a validation hard-reject `status: rejected` is stamped and the
/// reason returned. The `.curio/manifest.json` ownership oracle is
/// consulted when present (a malformed manifest is a warning — shape
/// detection still guards Curio files).
pub fn apply_proposal(
    vault: &Vault,
    proposals_dir: &str,
    id: &str,
) -> Result<ApplyReport, ApplyError> {
    let (mut proposal, patch) = load_proposal(vault, proposals_dir, id)?;
    if proposal.status != ProposalStatus::Open {
        return Err(ApplyError::NotOpen {
            id: id.to_owned(),
            status: status_str(proposal.status).to_owned(),
        });
    }
    let manifest = match CurioManifest::load(vault) {
        Ok(manifest) => manifest,
        Err(warning) => {
            tracing::warn!(%warning, "ignoring unreadable .curio manifest");
            None
        }
    };
    match validate_and_stage(vault, &proposal, &patch, manifest.as_ref()) {
        Ok(staged) => {
            write_all_or_revert(vault, &staged)?;
            store_proposal_status(vault, proposals_dir, &mut proposal, ProposalStatus::Applied)?;
            Ok(ApplyReport {
                id: proposal.id,
                title: proposal.title,
                files_written: staged.into_iter().map(|w| w.path).collect(),
                status: "applied".to_owned(),
            })
        }
        Err(reason) => {
            store_proposal_status(
                vault,
                proposals_dir,
                &mut proposal,
                ProposalStatus::Rejected,
            )?;
            Err(ApplyError::Rejected {
                id: id.to_owned(),
                reason,
            })
        }
    }
}

/// Errors from [`reject_proposal`].
#[derive(Debug, thiserror::Error)]
pub enum RejectError {
    /// The proposal is not `open` — transitions are one-way, so there is
    /// nothing to reject.
    #[error("proposal {id} is already {status}")]
    NotOpen { id: String, status: String },
    /// Reading/updating the stored proposal failed (environment — nothing
    /// was stamped).
    #[error(transparent)]
    Store(#[from] ProposalStoreError),
}

/// Reject an `open` proposal by human decision — stamp `rejected` *without*
/// attempting to apply it. This is the reviewer saying "no" up front, as
/// distinct from a *failed* [`apply_proposal`] (which also stamps
/// `rejected`, but only after the validator refuses). No files are touched.
///
/// Mirrors [`apply_proposal`]'s one-way guard: only an `open` proposal can
/// be rejected — `applied`/`rejected` are terminal, so a non-open proposal
/// returns [`RejectError::NotOpen`] and is left exactly as it was. (The
/// lower-level [`store_proposal_status`] has no such guard; this is the safe
/// verb to wire a UI button to.)
pub fn reject_proposal(
    vault: &Vault,
    proposals_dir: &str,
    id: &str,
) -> Result<Proposal, RejectError> {
    let (mut proposal, _patch) = load_proposal(vault, proposals_dir, id)?;
    if proposal.status != ProposalStatus::Open {
        return Err(RejectError::NotOpen {
            id: id.to_owned(),
            status: status_str(proposal.status).to_owned(),
        });
    }
    store_proposal_status(
        vault,
        proposals_dir,
        &mut proposal,
        ProposalStatus::Rejected,
    )?;
    Ok(proposal)
}

/// One validated write, staged in memory: the target path, its complete
/// new content, and the file's prior content (`None` = the patch creates
/// it) — enough to revert if a later write in the same apply fails.
#[derive(Debug)]
struct StagedWrite {
    path: String,
    content: String,
    prior: Option<String>,
}

/// Write every staged file, atomically each; if any write fails, revert
/// the files already written (restore prior content / remove creations)
/// so the apply stays all-or-nothing even against environment failures
/// (ENOSPC, permissions, a parent path turning out unwritable).
fn write_all_or_revert(vault: &Vault, staged: &[StagedWrite]) -> Result<(), VaultError> {
    for (i, w) in staged.iter().enumerate() {
        if let Err(err) = vault.write_atomic(&w.path, &w.content) {
            revert_written(vault, &staged[..i]);
            return Err(err);
        }
    }
    Ok(())
}

/// Best-effort rollback of already-written staged files, newest first. A
/// file that cannot be restored is warned about — never silently left.
fn revert_written(vault: &Vault, written: &[StagedWrite]) {
    for w in written.iter().rev() {
        let failure = match &w.prior {
            Some(old) => vault
                .write_atomic(&w.path, old)
                .err()
                .map(|e| e.to_string()),
            None => match vault.resolve(&w.path) {
                Ok(path) => std::fs::remove_file(&path).err().map(|e| e.to_string()),
                Err(e) => Some(e.to_string()),
            },
        };
        if let Some(err) = failure {
            tracing::warn!(path = %w.path, %err,
                "rollback of a partially-applied proposal could not restore this file");
        }
    }
}

/// The pure validator: parse, check every rule, and stage the resulting
/// writes WITHOUT touching the vault. `Err` is the human-readable
/// hard-reject reason.
fn validate_and_stage(
    vault: &Vault,
    proposal: &Proposal,
    patch: &str,
    manifest: Option<&CurioManifest>,
) -> Result<Vec<StagedWrite>, String> {
    let file_patches = parse_patch(patch).map_err(|e| e.to_string())?;
    if file_patches.is_empty() {
        return Err("changes.patch contains no file patches".to_owned());
    }
    let declared: BTreeSet<&str> = proposal.files.iter().map(String::as_str).collect();
    let mut staged: Vec<StagedWrite> = Vec::new();
    let mut new_identities: Vec<(String, String)> = Vec::new(); // (kp_id, path)

    for fp in &file_patches {
        let path = &fp.new_path;
        if let Some(old_path) = &fp.old_path
            && old_path != path
        {
            return Err(format!("renames are not supported: {old_path} -> {path}"));
        }
        if !declared.contains(path.as_str()) {
            return Err(format!(
                "changes.patch touches {path}, which proposal.json does not declare"
            ));
        }
        if staged.iter().any(|w| w.path == *path) {
            return Err(format!("duplicate file patch for {path}"));
        }
        // Rules 1 + 3: dot-dir policy, then vault path safety.
        validate_target_path(path).map_err(|e| e.to_string())?;
        let resolved = vault.resolve(path).map_err(|e| e.to_string())?;

        // Rule 4: strict clean application against the CURRENT tree.
        let old = if resolved.exists() {
            Some(vault.read(path).map_err(|e| e.to_string())?)
        } else {
            None
        };
        match (&fp.old_path, &old) {
            (None, Some(_)) => {
                return Err(format!(
                    "patch creates {path} but the file already exists (digests and other \
                     creations are create-only)"
                ));
            }
            (Some(_), None) => {
                return Err(format!("patch modifies {path} but the file does not exist"));
            }
            _ => {}
        }
        let new_content =
            apply_file_patch(old.as_deref().unwrap_or(""), fp).map_err(|e| e.to_string())?;

        // Rule 2: Curio surface protection — by shape or by the oracle.
        if let Some(old) = &old
            && (is_curio_shaped(path, old) || manifest.is_some_and(|m| m.owns(path)))
        {
            enforce_curio_preservation(path, old, &new_content).map_err(|e| e.to_string())?;
        }

        // Rule 5 (first half): collect the identities this proposal mints.
        // Per kp-note/v1 an identity is never absent — a plain new note
        // carries the implicit `path:<relpath>` identity, which collides
        // exactly like an explicit `kp_id` does.
        if old.is_none()
            && let Ok(note) = Note::parse(path.as_str(), &new_content)
        {
            let kp_id = note.kp_id().to_string();
            if let Some((_, first)) = new_identities.iter().find(|(id, _)| *id == kp_id) {
                return Err(format!(
                    "duplicate identity {kp_id} minted twice in one proposal ({first}, {path})"
                ));
            }
            new_identities.push((kp_id, path.clone()));
        }
        staged.push(StagedWrite {
            path: path.clone(),
            content: new_content,
            prior: old,
        });
    }

    // Rule 5 (second half): no minted identity may collide with a note
    // already in the vault. One walk covers every new file.
    if !new_identities.is_empty() {
        let existing = vault_identities(vault).map_err(|e| e.to_string())?;
        for (kp_id, path) in &new_identities {
            if let Some(existing_path) = existing.iter().find(|(id, _)| id == kp_id).map(|(_, p)| p)
            {
                return Err(format!(
                    "new note {path} duplicates existing identity {kp_id} ({existing_path})"
                ));
            }
        }
    }
    Ok(staged)
}

/// Every `(kp_id, path)` the vault currently holds — explicit
/// frontmatter identities AND the implicit `path:<relpath>` identity of
/// plain/foreign notes (kp-note/v1: identity is never absent, so a
/// minted `path:` id can collide with a plain note too). Unparseable
/// notes are skipped — a broken file cannot claim an identity.
fn vault_identities(vault: &Vault) -> Result<Vec<(String, String)>, VaultError> {
    let mut out = Vec::new();
    for path in vault.note_paths()? {
        let Ok(note) = vault.read_note(&path) else {
            continue;
        };
        out.push((note.kp_id().to_string(), path));
    }
    Ok(out)
}

/// The auto-apply gate: `true` only when the proposal purely ADDS files
/// under `digest_dir`, and every added file is a kp-note whose identity
/// is `kp:<uuidv7>`. Everything else waits for a human `curator apply`.
#[must_use]
pub fn auto_applicable(
    file_patches: &[FilePatch],
    digest_dir: &str,
    patch_contents: &[(String, String)],
) -> bool {
    let prefix = format!("{}/", digest_dir.trim_end_matches('/'));
    if file_patches.is_empty() {
        return false;
    }
    for fp in file_patches {
        if fp.old_path.is_some() || !fp.new_path.starts_with(&prefix) {
            return false;
        }
        let Some((_, content)) = patch_contents.iter().find(|(p, _)| *p == fp.new_path) else {
            return false;
        };
        let Ok(note) = Note::parse(fp.new_path.as_str(), content) else {
            return false;
        };
        let Frontmatter::Kp(fm) = &note.frontmatter else {
            return false;
        };
        match &fm.kp_id {
            curator_core::KpId::Kp(id) if is_uuid7(id) => {}
            _ => return false,
        }
    }
    true
}

/// Render one proposal for human review: metadata, rationale, files, and
/// the raw hunks.
#[must_use]
pub fn render_review(proposal: &Proposal, patch: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "proposal {} ({})\n",
        proposal.id,
        status_str(proposal.status)
    ));
    out.push_str(&format!("  title:     {}\n", proposal.title));
    out.push_str(&format!("  author:    {}\n", proposal.author));
    out.push_str(&format!("  created:   {}\n", proposal.created));
    out.push_str(&format!("  rationale: {}\n", proposal.rationale));
    out.push_str("  files:\n");
    for file in &proposal.files {
        out.push_str(&format!("    {file}\n"));
    }
    out.push('\n');
    out.push_str(patch);
    out
}

fn status_str(status: ProposalStatus) -> &'static str {
    match status {
        ProposalStatus::Open => "open",
        ProposalStatus::Applied => "applied",
        ProposalStatus::Rejected => "rejected",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use curator_core::{ProposalFile, create_proposal, list_proposals};

    fn tmp_vault(dir: &std::path::Path) -> Vault {
        let root = dir.join("vault");
        std::fs::create_dir_all(&root).expect("mkdir");
        Vault::open(&root).expect("open")
    }

    fn propose(vault: &Vault, files: &[(&str, &str)]) -> Proposal {
        let files: Vec<ProposalFile> = files
            .iter()
            .map(|(path, content)| ProposalFile {
                path: (*path).to_owned(),
                content: (*content).to_owned(),
            })
            .collect();
        create_proposal(
            vault,
            ".kp/proposals",
            "test",
            "a change",
            "because",
            &files,
        )
        .expect("creates")
    }

    #[test]
    fn applies_a_clean_proposal_and_stamps_applied() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        vault.write_atomic("n.md", "one\ntwo\n").expect("seed");
        let p = propose(&vault, &[("n.md", "one\n2\n"), ("new/idea.md", "# Idea\n")]);

        let report = apply_proposal(&vault, ".kp/proposals", &p.id).expect("applies");
        assert_eq!(report.files_written, vec!["n.md", "new/idea.md"]);
        assert_eq!(report.status, "applied");
        assert_eq!(vault.read("n.md").expect("read"), "one\n2\n");
        assert_eq!(vault.read("new/idea.md").expect("read"), "# Idea\n");
        let listed = list_proposals(&vault, ".kp/proposals").expect("lists");
        assert_eq!(listed[0].status, ProposalStatus::Applied);

        // Re-applying is refused: transitions are one-way.
        let err = apply_proposal(&vault, ".kp/proposals", &p.id).unwrap_err();
        assert!(matches!(err, ApplyError::NotOpen { ref status, .. } if status == "applied"));
    }

    #[test]
    fn drifted_vault_rejects_and_stamps_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        vault.write_atomic("n.md", "one\ntwo\n").expect("seed");
        let p = propose(&vault, &[("n.md", "one\n2\n")]);
        // The vault moves on before apply.
        vault.write_atomic("n.md", "one\nTWO\n").expect("drift");

        let err = apply_proposal(&vault, ".kp/proposals", &p.id).unwrap_err();
        assert!(
            matches!(err, ApplyError::Rejected { ref reason, .. } if reason.contains("cleanly")),
            "got {err}"
        );
        // Stamped rejected; the file is untouched.
        let listed = list_proposals(&vault, ".kp/proposals").expect("lists");
        assert_eq!(listed[0].status, ProposalStatus::Rejected);
        assert_eq!(vault.read("n.md").expect("read"), "one\nTWO\n");
        // And a rejected proposal cannot be applied later.
        let err = apply_proposal(&vault, ".kp/proposals", &p.id).unwrap_err();
        assert!(matches!(err, ApplyError::NotOpen { .. }));
    }

    #[test]
    fn reject_stamps_an_open_proposal_without_touching_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        let p = propose(&vault, &[("new/idea.md", "# Idea\n")]);

        let rejected = reject_proposal(&vault, ".kp/proposals", &p.id).expect("rejects");
        assert_eq!(rejected.status, ProposalStatus::Rejected);
        let listed = list_proposals(&vault, ".kp/proposals").expect("lists");
        assert_eq!(listed[0].status, ProposalStatus::Rejected);
        // Reject never applies — the target file was not written.
        assert!(vault.read("new/idea.md").is_err());
        // And a rejected proposal cannot then be applied (one-way).
        let err = apply_proposal(&vault, ".kp/proposals", &p.id).unwrap_err();
        assert!(matches!(err, ApplyError::NotOpen { .. }));
    }

    #[test]
    fn reject_refuses_a_non_open_proposal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        vault.write_atomic("n.md", "one\ntwo\n").expect("seed");
        let p = propose(&vault, &[("n.md", "one\n2\n")]);
        apply_proposal(&vault, ".kp/proposals", &p.id).expect("applies");

        // Already applied → reject is refused and the status is unchanged.
        let err = reject_proposal(&vault, ".kp/proposals", &p.id).unwrap_err();
        assert!(matches!(err, RejectError::NotOpen { ref status, .. } if status == "applied"));
        let listed = list_proposals(&vault, ".kp/proposals").expect("lists");
        assert_eq!(listed[0].status, ProposalStatus::Applied);
    }

    /// A hand-crafted proposal dir (an agent writing files directly,
    /// bypassing create_proposal) still hits every validator rule.
    fn plant_proposal(vault: &Vault, id: &str, files: &[&str], patch: &str) {
        let proposal = serde_json::json!({
            "schema": "proposals/v1",
            "id": id,
            "created": "2026-07-03T09:15:00Z",
            "author": "rogue-agent",
            "title": "hand-crafted",
            "rationale": "trust me",
            "status": "open",
            "files": files,
        });
        let dir = vault.root().join(".kp/proposals").join(id);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join("proposal.json"),
            serde_json::to_string_pretty(&proposal).expect("json"),
        )
        .expect("write");
        std::fs::write(dir.join("changes.patch"), patch).expect("write");
    }

    #[test]
    fn hand_crafted_curio_path_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        plant_proposal(
            &vault,
            "01AAAAAAAAAAAAAAAAAAAAAAAA",
            &[".curio/manifest.json"],
            "--- /dev/null\n+++ b/.curio/manifest.json\n@@ -0,0 +1,1 @@\n+{}\n",
        );
        let err =
            apply_proposal(&vault, ".kp/proposals", "01AAAAAAAAAAAAAAAAAAAAAAAA").unwrap_err();
        assert!(
            matches!(err, ApplyError::Rejected { ref reason, .. } if reason.contains(".curio")),
            "got {err}"
        );
        assert!(!vault.root().join(".curio/manifest.json").exists());
    }

    #[test]
    fn hand_crafted_nested_dot_path_is_rejected() {
        // The walker/index/audit surface skips dot-named entries at EVERY
        // depth, so a nested dot path would be a plane-invisible write —
        // the validator must reject it wherever the dot component sits.
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        plant_proposal(
            &vault,
            "01EEEEEEEEEEEEEEEEEEEEEEEE",
            &["notes/.trash/exfil.md"],
            "--- /dev/null\n+++ b/notes/.trash/exfil.md\n@@ -0,0 +1,1 @@\n+hidden\n",
        );
        let err =
            apply_proposal(&vault, ".kp/proposals", "01EEEEEEEEEEEEEEEEEEEEEEEE").unwrap_err();
        assert!(
            matches!(err, ApplyError::Rejected { ref reason, .. } if reason.contains("dot-directory")),
            "got {err}"
        );
        assert!(!vault.root().join("notes/.trash/exfil.md").exists());
    }

    #[test]
    fn hand_crafted_traversal_and_undeclared_paths_are_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        plant_proposal(
            &vault,
            "01BBBBBBBBBBBBBBBBBBBBBBBB",
            &["../escape.md"],
            "--- /dev/null\n+++ b/../escape.md\n@@ -0,0 +1,1 @@\n+gotcha\n",
        );
        let err =
            apply_proposal(&vault, ".kp/proposals", "01BBBBBBBBBBBBBBBBBBBBBBBB").unwrap_err();
        assert!(matches!(err, ApplyError::Rejected { .. }), "got {err}");
        assert!(!dir.path().join("escape.md").exists());

        // Patch touches a path proposal.json does not declare.
        plant_proposal(
            &vault,
            "01CCCCCCCCCCCCCCCCCCCCCCCC",
            &["declared.md"],
            "--- /dev/null\n+++ b/undeclared.md\n@@ -0,0 +1,1 @@\n+sneaky\n",
        );
        let err =
            apply_proposal(&vault, ".kp/proposals", "01CCCCCCCCCCCCCCCCCCCCCCCC").unwrap_err();
        assert!(
            matches!(err, ApplyError::Rejected { ref reason, .. } if reason.contains("does not declare")),
            "got {err}"
        );
    }

    const MANAGED_NOTE: &str = "---\nschema: curio.frontmatter.v1\ncurio_id: \"0197\"\ntitle: T\n---\n<!-- curio:managed:begin v1 -->\nmachine text\n<!-- curio:managed:end -->\ncompanion\n";

    #[test]
    fn managed_region_edits_are_rejected_at_apply_time() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        vault
            .write_atomic("curio/a.md", MANAGED_NOTE)
            .expect("seed");
        // Hand-craft the patch (create_proposal would already refuse).
        let tampered = MANAGED_NOTE.replace("machine text", "edited");
        let mut patch = String::from("--- a/curio/a.md\n+++ b/curio/a.md\n@@ -6,3 +6,3 @@\n");
        patch.push_str(" <!-- curio:managed:begin v1 -->\n-machine text\n+edited\n <!-- curio:managed:end -->\n");
        plant_proposal(
            &vault,
            "01DDDDDDDDDDDDDDDDDDDDDDDD",
            &["curio/a.md"],
            &patch,
        );
        let err =
            apply_proposal(&vault, ".kp/proposals", "01DDDDDDDDDDDDDDDDDDDDDDDD").unwrap_err();
        assert!(
            matches!(err, ApplyError::Rejected { ref reason, .. } if reason.contains("managed region")),
            "got {err}"
        );
        assert_eq!(vault.read("curio/a.md").expect("read"), MANAGED_NOTE);
        let _ = tampered;
    }

    #[test]
    fn manifest_oracle_guards_files_without_curio_shape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        // A note the manifest claims but whose content has no markers and
        // foreign (non-curio-schema) frontmatter.
        vault
            .write_atomic(
                "curio/plain.md",
                "---\ntitle: claimed\n---\nexported text\n",
            )
            .expect("seed");
        std::fs::create_dir_all(vault.root().join(".curio")).expect("mkdir");
        std::fs::write(
            vault.root().join(".curio/manifest.json"),
            "{\"schema\":\"curio.manifest.v1\",\"notes\":{\"0197\":{\"path\":\"curio/plain.md\",\"checksum\":\"sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08\",\"exported_at\":\"2026-07-03T09:15:00.123Z\"}}}",
        )
        .expect("manifest");

        // Rewriting its frontmatter must be rejected via the oracle…
        let p = propose(
            &vault,
            &[(
                "curio/plain.md",
                "---\ntitle: renamed\n---\nexported text\n",
            )],
        );
        let err = apply_proposal(&vault, ".kp/proposals", &p.id).unwrap_err();
        assert!(
            matches!(err, ApplyError::Rejected { ref reason, .. } if reason.contains("title")),
            "got {err}"
        );
        // …while additive companion content below is fine.
        let p = propose(
            &vault,
            &[(
                "curio/plain.md",
                "---\ntitle: claimed\n---\nexported text\n\nmy thoughts\n",
            )],
        );
        apply_proposal(&vault, ".kp/proposals", &p.id).expect("companion append applies");
    }

    #[test]
    fn duplicate_identity_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        vault
            .write_atomic(
                "existing.md",
                "---\nkp_id: \"kp:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d\"\nkp_schema: kp-note/v1\ntitle: E\n---\nbody\n",
            )
            .expect("seed");
        let p = propose(
            &vault,
            &[(
                "new.md",
                "---\nkp_id: \"kp:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d\"\nkp_schema: kp-note/v1\ntitle: N\n---\nother\n",
            )],
        );
        let err = apply_proposal(&vault, ".kp/proposals", &p.id).unwrap_err();
        assert!(
            matches!(err, ApplyError::Rejected { ref reason, .. } if reason.contains("duplicates existing identity")),
            "got {err}"
        );
        assert!(!vault.root().join("new.md").exists());
    }

    #[test]
    fn duplicate_implicit_path_identity_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        // A PLAIN note holds the implicit identity path:existing.md.
        vault
            .write_atomic("existing.md", "# Plain\n\nno frontmatter\n")
            .expect("seed");
        // A new note explicitly claiming that implicit identity collides.
        let p = propose(
            &vault,
            &[(
                "new.md",
                "---\nkp_id: \"path:existing.md\"\nkp_schema: kp-note/v1\ntitle: N\n---\nbody\n",
            )],
        );
        let err = apply_proposal(&vault, ".kp/proposals", &p.id).unwrap_err();
        assert!(
            matches!(err, ApplyError::Rejected { ref reason, .. } if reason.contains("duplicates existing identity")),
            "got {err}"
        );
        assert!(!vault.root().join("new.md").exists());

        // Within one proposal: a plain new file's implicit path: identity
        // collides with another new file explicitly claiming it.
        let p = propose(
            &vault,
            &[
                ("idea.md", "# Idea\n"),
                (
                    "other.md",
                    "---\nkp_id: \"path:idea.md\"\nkp_schema: kp-note/v1\ntitle: O\n---\nbody\n",
                ),
            ],
        );
        let err = apply_proposal(&vault, ".kp/proposals", &p.id).unwrap_err();
        assert!(
            matches!(err, ApplyError::Rejected { ref reason, .. } if reason.contains("minted twice")),
            "got {err}"
        );
        assert!(!vault.root().join("idea.md").exists());
        assert!(!vault.root().join("other.md").exists());
    }

    #[test]
    fn mid_write_failure_reverts_and_leaves_the_proposal_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        vault.write_atomic("a.md", "original\n").expect("seed");
        // "sub" is a regular FILE, so creating sub/two.md passes
        // validation (nothing checks parents) but fails at write time —
        // an environment failure, not a validation reject.
        std::fs::write(vault.root().join("sub"), "in the way").expect("plant");
        let p = propose(
            &vault,
            &[("a.md", "rewritten\n"), ("sub/two.md", "new file\n")],
        );

        let err = apply_proposal(&vault, ".kp/proposals", &p.id).unwrap_err();
        assert!(matches!(err, ApplyError::Vault(_)), "got {err}");
        // All-or-nothing: the first write was reverted…
        assert_eq!(vault.read("a.md").expect("read"), "original\n");
        // …and the proposal is still open (NOT stamped rejected), so a
        // retry after fixing the environment succeeds.
        let listed = list_proposals(&vault, ".kp/proposals").expect("lists");
        assert_eq!(listed[0].status, ProposalStatus::Open);

        std::fs::remove_file(vault.root().join("sub")).expect("unplant");
        let report = apply_proposal(&vault, ".kp/proposals", &p.id).expect("retry applies");
        assert_eq!(report.files_written, vec!["a.md", "sub/two.md"]);
        assert_eq!(vault.read("a.md").expect("read"), "rewritten\n");
        assert_eq!(vault.read("sub/two.md").expect("read"), "new file\n");
    }

    #[test]
    fn rejection_writes_nothing_even_when_other_files_are_clean() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        vault.write_atomic("good.md", "ok\n").expect("seed");
        vault.write_atomic("drifts.md", "original\n").expect("seed");
        let p = propose(
            &vault,
            &[("good.md", "ok\nmore\n"), ("drifts.md", "changed\n")],
        );
        vault
            .write_atomic("drifts.md", "moved on\n")
            .expect("drift");

        let err = apply_proposal(&vault, ".kp/proposals", &p.id).unwrap_err();
        assert!(matches!(err, ApplyError::Rejected { .. }));
        // Validation is all-or-nothing: the clean file was not written.
        assert_eq!(vault.read("good.md").expect("read"), "ok\n");
    }

    #[test]
    fn auto_apply_gate_admits_only_pure_digest_additions() {
        let digest_note = "---\nkp_id: \"kp:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d\"\nkp_schema: kp-note/v1\ntitle: Daily digest 2026-07-03\ntags:\n- digest\n---\nbody\n";
        let creation = |path: &str| FilePatch {
            old_path: None,
            new_path: path.to_owned(),
            hunks: Vec::new(),
        };
        let contents = |path: &str, content: &str| vec![(path.to_owned(), content.to_owned())];

        // The happy shape.
        assert!(auto_applicable(
            &[creation("digests/2026-07-03.md")],
            "digests",
            &contents("digests/2026-07-03.md", digest_note),
        ));
        // Outside the digest dir.
        assert!(!auto_applicable(
            &[creation("notes/x.md")],
            "digests",
            &contents("notes/x.md", digest_note),
        ));
        // A modification, not an addition.
        let modification = FilePatch {
            old_path: Some("digests/2026-07-03.md".to_owned()),
            new_path: "digests/2026-07-03.md".to_owned(),
            hunks: Vec::new(),
        };
        assert!(!auto_applicable(
            &[modification],
            "digests",
            &contents("digests/2026-07-03.md", digest_note),
        ));
        // Wrong namespace / not a uuid7 / no frontmatter.
        for bad in [
            "---\nkp_id: \"curio:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d\"\nkp_schema: kp-note/v1\ntitle: T\n---\nb\n",
            "---\nkp_id: \"kp:not-a-uuid\"\nkp_schema: kp-note/v1\ntitle: T\n---\nb\n",
            "just a body\n",
        ] {
            assert!(
                !auto_applicable(
                    &[creation("digests/2026-07-03.md")],
                    "digests",
                    &contents("digests/2026-07-03.md", bad),
                ),
                "{bad:?} must not auto-apply"
            );
        }
        // Empty proposals never auto-apply.
        assert!(!auto_applicable(&[], "digests", &[]));
    }

    #[test]
    fn review_render_names_everything_reviewers_need() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = tmp_vault(dir.path());
        let p = propose(&vault, &[("notes/idea.md", "# Idea\n")]);
        let (proposal, patch) = load_proposal(&vault, ".kp/proposals", &p.id).expect("loads");
        let render = render_review(&proposal, &patch);
        for needle in [
            p.id.as_str(),
            "open",
            "a change",
            "because",
            "notes/idea.md",
            "+# Idea",
        ] {
            assert!(render.contains(needle), "missing {needle:?} in:\n{render}");
        }
    }
}
