//! Streamable-HTTP transport — the opt-in network deployment.
//!
//! Contract binding rule 4 (`contracts/mcp/v1.md`): when `transport =
//! "http"`, the bearer token is REQUIRED — there is no unauthenticated
//! network mode. The token arrives via env indirection only
//! (`[mcp].bearer_token_env` names the variable, per `kp-config/v1`);
//! startup refuses without it, and every request must present
//! `Authorization: Bearer <token>`, compared in constant time (SHA-256
//! both sides, then a branch-free fold — no length or prefix leaks).

use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use curator_core::config::{McpConfig, secret_with};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::{
    StreamableHttpServerConfig, StreamableHttpService,
};
use sha2::{Digest, Sha256};

use crate::engine::KpEngine;
use crate::server::KpMcpServer;

/// The MCP endpoint path.
pub const MCP_PATH: &str = "/mcp";

/// Errors from configuring or running the HTTP transport.
#[derive(Debug, thiserror::Error)]
pub enum HttpServeError {
    /// `[mcp].bearer_token_env` is empty — the config names no variable.
    #[error(
        "[mcp].bearer_token_env is empty — http transport requires a bearer token \
         (there is no unauthenticated network mode)"
    )]
    NoTokenVariable,
    /// The named variable is unset or empty in the environment.
    #[error(
        "refusing to start: http transport requires a bearer token, but ${var} is \
         unset or empty (there is no unauthenticated network mode)"
    )]
    TokenMissing { var: String },
    /// The bind address could not be bound.
    #[error("cannot bind {addr}: {source}")]
    Bind {
        addr: String,
        #[source]
        source: std::io::Error,
    },
    /// The HTTP server failed while serving.
    #[error("http serve: {0}")]
    Serve(#[source] std::io::Error),
}

/// Resolve the mandatory bearer token from the environment variable named
/// by `[mcp].bearer_token_env`. Refuses (never defaults) when the name is
/// empty or the variable is unset/empty.
pub fn resolve_bearer_token(mcp: &McpConfig) -> Result<String, HttpServeError> {
    resolve_bearer_token_with(mcp, |name| std::env::var(name).ok())
}

/// [`resolve_bearer_token`] against an explicit lookup — the pure core,
/// for tests.
pub fn resolve_bearer_token_with(
    mcp: &McpConfig,
    get: impl Fn(&str) -> Option<String>,
) -> Result<String, HttpServeError> {
    if mcp.bearer_token_env.is_empty() {
        return Err(HttpServeError::NoTokenVariable);
    }
    secret_with(&mcp.bearer_token_env, get).ok_or_else(|| HttpServeError::TokenMissing {
        var: mcp.bearer_token_env.clone(),
    })
}

#[derive(Clone)]
struct AuthState {
    /// SHA-256 of the expected token — comparisons run digest-vs-digest,
    /// so candidate length never shapes timing.
    expected_digest: Arc<[u8; 32]>,
}

impl std::fmt::Debug for AuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthState(<digest>)") // never print token material
    }
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Branch-free 32-byte comparison.
fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Does the request carry `Authorization: Bearer <expected>`?
fn authorized(req: &Request, state: &AuthState) -> bool {
    let Some(value) = req.headers().get(header::AUTHORIZATION) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    let Some(candidate) = value.strip_prefix("Bearer ") else {
        return false;
    };
    constant_time_eq(&digest(candidate.as_bytes()), &state.expected_digest)
}

async fn require_bearer(State(state): State<AuthState>, req: Request, next: Next) -> Response {
    if authorized(&req, &state) {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
        )
            .into_response()
    }
}

/// The HTTP application: rmcp's streamable-HTTP service at [`MCP_PATH`],
/// with the bearer gate in front of EVERY route (a request that misses
/// the endpoint still gets 401 before 404 — the server's existence is
/// all an unauthenticated caller learns).
pub fn http_app(engine: Arc<KpEngine>, token: &str) -> Router {
    // Bearer auth gates every request, which already defeats the DNS
    // rebinding attack the default loopback Host allowlist exists for —
    // and the bind address comes from config, so non-loopback deployments
    // must not be silently rejected on the Host header.
    let config = StreamableHttpServerConfig::default().disable_allowed_hosts();
    let service = StreamableHttpService::new(
        move || Ok(KpMcpServer::new(engine.clone())),
        LocalSessionManager::default().into(),
        config,
    );
    let auth = AuthState {
        expected_digest: Arc::new(digest(token.as_bytes())),
    };
    Router::new()
        .nest_service(MCP_PATH, service)
        .layer(middleware::from_fn_with_state(auth, require_bearer))
}

/// Bind `addr` and serve the streamable-HTTP transport until the process
/// is stopped.
pub async fn serve_http(
    engine: Arc<KpEngine>,
    addr: &str,
    token: &str,
) -> Result<(), HttpServeError> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|source| HttpServeError::Bind {
            addr: addr.to_owned(),
            source,
        })?;
    tracing::info!(%addr, path = MCP_PATH, "curator-mcp http transport listening");
    axum::serve(listener, http_app(engine, token))
        .await
        .map_err(HttpServeError::Serve)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_agrees_with_plain_eq() {
        let a = digest(b"token-a");
        let b = digest(b"token-b");
        assert!(constant_time_eq(&a, &a));
        assert!(!constant_time_eq(&a, &b));
    }

    #[test]
    fn token_resolution_refuses_missing_or_empty() {
        let mcp = McpConfig::default(); // bearer_token_env = "KP_MCP_TOKEN"
        let err = resolve_bearer_token_with(&mcp, |_| None).unwrap_err();
        assert!(matches!(err, HttpServeError::TokenMissing { ref var } if var == "KP_MCP_TOKEN"));
        assert!(err.to_string().contains("refusing to start"));

        // Set-but-empty is refused too.
        let err = resolve_bearer_token_with(&mcp, |_| Some(String::new())).unwrap_err();
        assert!(matches!(err, HttpServeError::TokenMissing { .. }));

        // An empty variable NAME is a config error, not a lookup miss.
        let no_var = McpConfig {
            bearer_token_env: String::new(),
            ..Default::default()
        };
        assert!(matches!(
            resolve_bearer_token_with(&no_var, |_| Some("x".into())).unwrap_err(),
            HttpServeError::NoTokenVariable
        ));

        // And a present token resolves.
        let token = resolve_bearer_token_with(&mcp, |_| Some("s3cret".into())).expect("resolves");
        assert_eq!(token, "s3cret");
    }
}
