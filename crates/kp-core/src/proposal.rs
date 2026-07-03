//! `proposals/v1` — the ONLY write path for agents
//! (contract: `contracts/proposals/v1.md`).
//!
//! Local-first and forge-free: the validator works with no git remote at
//! all. Layout: `<vault>/.kp/proposals/<ULID>/` containing `proposal.json`
//! (this type) + `changes.patch` (unified diff against the vault tree).

use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
