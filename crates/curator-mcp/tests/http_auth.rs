//! HTTP transport auth: the bearer gate in front of the MCP endpoint.
//!
//! The axum router is driven in-process with `tower::ServiceExt::oneshot`
//! — request in, response out, no sockets — so these tests stay hermetic
//! while exercising the exact middleware stack `curator mcp serve --http`
//! runs. Startup refusal (no token in the environment) is covered by the
//! `resolve_bearer_token` unit tests plus the CLI integration test.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use curator_core::KpConfig;
use curator_mcp::{KpEngine, http_app};
use tower::ServiceExt;

const TOKEN: &str = "test-token-1234";

fn app(dir: &std::path::Path) -> axum::Router {
    let vault = dir.join("vault");
    std::fs::create_dir_all(&vault).expect("mkdir vault");
    let config = KpConfig::from_toml_str(&format!(
        "schema = \"kp-config/v1\"\n\
         [vault]\npath = \"{}\"\n\
         [index]\npath = \"{}\"\nembedder = \"hash\"\n",
        vault.display(),
        dir.join("index.db").display(),
    ))
    .expect("config parses");
    let engine = Arc::new(KpEngine::from_config(config).expect("engine"));
    http_app(engine, TOKEN)
}

fn initialize_request(auth: Option<&str>) -> Request<Body> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "auth-test", "version": "0"}
        }
    });
    let mut builder = Request::builder()
        .method("POST")
        .uri("/mcp")
        // tower::oneshot skips hyper, so supply what any real HTTP/1.1
        // client sends: the service requires a Host header.
        .header(header::HOST, "localhost")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream");
    if let Some(auth) = auth {
        builder = builder.header(header::AUTHORIZATION, auth);
    }
    builder
        .body(Body::from(body.to_string()))
        .expect("request builds")
}

#[tokio::test]
async fn missing_bearer_is_401_with_challenge() {
    let dir = tempfile::tempdir().expect("tempdir");
    let response = app(dir.path())
        .oneshot(initialize_request(None))
        .await
        .expect("router serves");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .expect("challenge header")
            .to_str()
            .expect("ascii"),
        "Bearer"
    );
}

#[tokio::test]
async fn wrong_bearer_is_401() {
    let dir = tempfile::tempdir().expect("tempdir");
    for bad in [
        "Bearer wrong-token",
        "Bearer ",
        "Bearer test-token-123",   // prefix of the real token
        "Bearer test-token-12345", // real token plus a suffix
        "bearer test-token-1234",  // scheme is case-sensitive here
        "Basic dGVzdDp0ZXN0",
    ] {
        let response = app(dir.path())
            .oneshot(initialize_request(Some(bad)))
            .await
            .expect("router serves");
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{bad:?} must not pass"
        );
    }
}

#[tokio::test]
async fn correct_bearer_reaches_the_mcp_service() {
    use http_body_util::BodyExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let response = app(dir.path())
        .oneshot(initialize_request(Some(&format!("Bearer {TOKEN}"))))
        .await
        .expect("router serves");
    // Past the gate, the MCP service answers the initialize request.
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(
        status,
        StatusCode::OK,
        "body: {}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
async fn the_gate_covers_every_route_and_method() {
    let dir = tempfile::tempdir().expect("tempdir");
    for (method, uri) in [("GET", "/mcp"), ("DELETE", "/mcp"), ("GET", "/anything")] {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .expect("request builds");
        let response = app(dir.path())
            .oneshot(request)
            .await
            .expect("router serves");
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {uri} must be gated"
        );
    }
}
