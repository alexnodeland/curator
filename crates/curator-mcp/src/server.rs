//! The MCP server: exactly the six v1 tools, routed by rmcp
//! (contract: `contracts/mcp/v1.md`).
//!
//! Tool handlers are thin: parse typed arguments, call the shared
//! [`KpEngine`], wrap the typed output in [`Json`] so results carry both
//! `structured_content` and an advertised output schema. Handlers are
//! synchronous on purpose — every operation is a short local SQLite read
//! (or one proposal write); there is nothing to await.

use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};

use crate::engine::KpEngine;
use crate::types::{
    DigestOutput, GetNoteArgs, NoteOutput, ProposeArgs, ProposeOutput, RecentArgs, RecentOutput,
    RelatedArgs, RelatedOutput, SearchArgs, SearchOutput,
};

/// The MCP surface v1 handler.
#[derive(Debug, Clone)]
pub struct KpMcpServer {
    engine: Arc<KpEngine>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl KpMcpServer {
    /// Wrap the shared engine as an MCP server.
    #[must_use]
    pub fn new(engine: Arc<KpEngine>) -> Self {
        Self {
            engine,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "kp_search",
        description = "Search the knowledge plane. Returns ranked notes (id, title, path, \
                       snippet, score) for a free-text query; mode is hybrid (default), \
                       vector, or fts; k defaults to 10."
    )]
    fn kp_search(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<Json<SearchOutput>, String> {
        self.engine
            .search(&args.query, args.k, args.mode)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(
        name = "kp_get_note",
        description = "Fetch one note by id (any identity namespace: curio: | zotero: | kp: \
                       | path:). Returns full content, frontmatter, and index metadata."
    )]
    fn kp_get_note(
        &self,
        Parameters(args): Parameters<GetNoteArgs>,
    ) -> Result<Json<NoteOutput>, String> {
        self.engine
            .get_note(&args.id)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(
        name = "kp_related",
        description = "Embedding-nearest notes to an existing note (by id, any namespace); \
                       k defaults to 10. The note itself is excluded."
    )]
    fn kp_related(
        &self,
        Parameters(args): Parameters<RelatedArgs>,
    ) -> Result<Json<RelatedOutput>, String> {
        self.engine
            .related(&args.id, args.k)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(
        name = "kp_recent",
        description = "Recently ingested or changed notes; days defaults to 7, kind \
                       optionally filters by identity namespace (curio | zotero | kp | path)."
    )]
    fn kp_recent(
        &self,
        Parameters(args): Parameters<RecentArgs>,
    ) -> Result<Json<RecentOutput>, String> {
        self.engine
            .recent(args.days, args.kind)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(
        name = "kp_propose",
        description = "Propose vault changes (the ONLY write verb). Creates a proposals/v1 \
                       changeset — title, rationale, and the full new content of each file — \
                       for human review; nothing is applied directly."
    )]
    fn kp_propose(
        &self,
        Parameters(args): Parameters<ProposeArgs>,
    ) -> Result<Json<ProposeOutput>, String> {
        self.engine
            .propose(&args.title, &args.rationale, &args.files)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[tool(
        name = "kp_digest_latest",
        description = "The latest librarian digest note, or null when none exists yet."
    )]
    fn kp_digest_latest(&self) -> Result<Json<DigestOutput>, String> {
        self.engine
            .digest_latest()
            .map(Json)
            .map_err(|e| e.to_string())
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for KpMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Curator MCP surface v1: kp_search / kp_get_note / kp_related / \
             kp_recent read the index; kp_propose is the only write verb (all writes \
             ride proposals/v1 and wait for human application); kp_digest_latest \
             returns the newest librarian digest.",
        )
    }
}

/// Errors from running a transport to completion.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// The MCP handshake failed. Boxed: the initialize error is large
    /// and this path is terminal, never hot.
    #[error("mcp initialize: {0}")]
    Initialize(#[from] Box<rmcp::service::ServerInitializeError>),
    /// The serving task panicked or was cancelled.
    #[error("mcp serving task: {0}")]
    Join(#[from] tokio::task::JoinError),
}

/// Serve one MCP session over stdio (the default transport) until the
/// client disconnects.
pub async fn serve_stdio(engine: Arc<KpEngine>) -> Result<(), ServeError> {
    let running = KpMcpServer::new(engine)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(Box::new)?;
    running.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TOOLS_V1;

    #[test]
    fn the_router_exposes_exactly_the_six_contract_tools() {
        let tools = KpMcpServer::tool_router();
        let names: Vec<String> = tools
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        let mut expected: Vec<String> = TOOLS_V1.iter().map(|s| (*s).to_owned()).collect();
        expected.sort();
        assert_eq!(names, expected, "tool names ARE the contract");
    }

    #[test]
    fn every_tool_advertises_input_and_output_schemas() {
        for tool in KpMcpServer::tool_router().list_all() {
            assert!(
                tool.output_schema.is_some(),
                "{} lacks an output schema",
                tool.name
            );
            assert!(
                tool.description.is_some(),
                "{} lacks a description",
                tool.name
            );
        }
    }
}
