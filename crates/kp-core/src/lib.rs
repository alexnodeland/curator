//! Knowledge Plane core.
//!
//! The bottom of the workspace dependency graph: the vault model, the
//! published-contract data types, identity minting/resolution, and the
//! `proposals/v1` validator live here. Contract discipline is absolute —
//! the documents under `contracts/` are the API; this code conforms to
//! them, never the other way around.

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
pub use proposal::{Proposal, ProposalFile, ProposalStatus, ProposalWriteError, create_proposal};
pub use vault::{Vault, VaultError};
