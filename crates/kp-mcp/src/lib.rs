//! Knowledge Plane MCP server — MCP surface v1
//! (contract: `contracts/mcp/v1.md`).
//!
//! One combined entrypoint: stdio by default, streamable HTTP + bearer
//! token optional. Retrieval is in-process — this crate links `kp-index`
//! directly; there is no internal network API. Tool names/shapes ARE the
//! contract: adding tools is a minor version, changing shapes is a major.
//!
//! Layering: [`engine::KpEngine`] is the ONE query layer (the CLI's
//! `search`/`get`/`related`/`recent` commands call it too, so CLI tests
//! cover tool logic); [`server::KpMcpServer`] routes the six tools over
//! it; [`http`] adds the bearer-gated network transport.

pub mod engine;
pub mod http;
pub mod server;
pub mod types;

pub use engine::{EngineError, KpEngine};
pub use http::{HttpServeError, http_app, resolve_bearer_token, serve_http};
pub use server::{KpMcpServer, ServeError, serve_stdio};

/// The six v1 tool names, exactly as published.
pub const TOOLS_V1: [&str; 6] = [
    "kp_search",
    "kp_get_note",
    "kp_related",
    "kp_recent",
    "kp_propose",
    "kp_digest_latest",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn tool_names_are_unique_and_namespaced() {
        let set: HashSet<&str> = TOOLS_V1.iter().copied().collect();
        assert_eq!(set.len(), TOOLS_V1.len());
        assert!(TOOLS_V1.iter().all(|t| t.starts_with("kp_")));
    }

    /// The tool table in `contracts/mcp/v1.md` IS the contract. `TOOLS_V1`
    /// mirrors it, and the router test pins the router to `TOOLS_V1` — this
    /// test closes the loop so the published doc can never silently drift
    /// from the served surface.
    #[test]
    fn the_published_tool_table_matches_tools_v1() {
        let doc = include_str!("../../../contracts/mcp/v1.md");
        let documented: Vec<&str> = doc
            .lines()
            .filter_map(|line| {
                // Tool-table rows only: `| \`kp_...\` | args | returns |`.
                let name = line.trim().strip_prefix("| `")?.split('`').next()?;
                name.starts_with("kp_").then_some(name)
            })
            .collect();
        assert_eq!(
            documented,
            TOOLS_V1.to_vec(),
            "contracts/mcp/v1.md tool table drifted from TOOLS_V1 (order is part of the doc)"
        );
    }
}
