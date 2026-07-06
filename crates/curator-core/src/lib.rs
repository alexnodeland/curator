//! Curator core.
//!
//! The bottom of the workspace dependency graph: the vault model, the
//! published-contract data types, identity minting/resolution, and the
//! `proposals/v1` validator live here. Contract discipline is absolute —
//! the documents under `contracts/` are the API; this code conforms to
//! them, never the other way around.

// curator-core is the contract crate: every public item is API surface and
// documents itself. `warn` here becomes a hard error under clippy's
// `-D warnings` in the gate suite.
#![warn(missing_docs)]

pub mod checksum;
pub mod config;
pub mod id;
pub mod managed;
pub mod note;
pub mod proposal;
pub mod time;
pub mod vault;

pub use checksum::{Checksum, ChecksumError};
pub use config::{ConfigError, KpConfig};
pub use id::{IdError, KpId};
pub use note::{Frontmatter, Note, NoteError, NoteFrontmatter};
pub use proposal::{
    Proposal, ProposalFile, ProposalStatus, ProposalStoreError, ProposalWriteError,
    create_proposal, enforce_curio_preservation, is_curio_shaped, list_proposals, load_proposal,
    proposal_rel_dir, store_proposal_status, validate_target_path,
};
pub use vault::{Vault, VaultError};
