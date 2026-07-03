//! Knowledge Plane core.
//!
//! The bottom of the workspace dependency graph: the vault model, the
//! published-contract data types, identity minting/resolution, and the
//! `proposals/v1` validator live here. Contract discipline is absolute —
//! the documents under `contracts/` are the API; this code conforms to
//! them, never the other way around.

pub mod config;
pub mod id;
pub mod note;
pub mod proposal;

pub use config::KpConfig;
pub use id::KpId;
pub use note::NoteFrontmatter;
pub use proposal::{Proposal, ProposalStatus};
