//! In-process MCP client<->server round trips for every v1 tool.
//!
//! A real rmcp client talks to the real [`KpMcpServer`] over tokio duplex
//! pipes — the full protocol path (initialize, tools/list, tools/call)
//! with zero sockets and zero processes. The index is seeded with the
//! deterministic hash embedder; everything is hermetic.

use std::path::Path;
use std::sync::Arc;

use curator_core::KpConfig;
use curator_core::note::Note;
use curator_index::{ChunkParams, HashEmbedder, Index};
use curator_mcp::types::{
    DigestOutput, NoteOutput, ProposeOutput, RecentOutput, RelatedOutput, SearchOutput,
};
use curator_mcp::{KpEngine, KpMcpServer, TOOLS_V1};
use rmcp::model::{CallToolRequestParams, CallToolResult, ClientInfo};
use rmcp::service::{RoleClient, RunningService};
use rmcp::{ClientHandler, ServiceExt};
use serde_json::json;

#[derive(Debug, Clone, Default)]
struct TestClient;

impl ClientHandler for TestClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::default()
    }
}

/// Seed a vault + hash-embedded index under `dir`; returns the config.
fn seed(dir: &Path, with_digest: bool) -> KpConfig {
    let vault = dir.join("vault");
    std::fs::create_dir_all(&vault).expect("mkdir vault");
    let index_path = dir.join("index.db");

    let e = HashEmbedder::default();
    let mut index = Index::create(&index_path, &e, 1).expect("create index");
    let params = ChunkParams {
        tokens: 16,
        overlap: 2,
    };
    let mut notes = vec![
        (
            "rust/db.md",
            "---\nkp_id: \"kp:aaa\"\nkp_schema: kp-note/v1\ntitle: Rust databases\n\
             tags: [rust, databases]\nsource: \"https://example.com/rust-db\"\n\
             updated: 2026-07-01T00:00:00Z\n---\n\
             rust database embedded sqlite storage engine queries indexes design\n"
                .to_owned(),
        ),
        (
            "rust/async.md",
            "---\nkp_id: \"curio:bbb\"\nkp_schema: kp-note/v1\ntitle: Async rust\n---\n\
             rust database embedded sqlite storage engine queries indexes async\n"
                .to_owned(),
        ),
        (
            "cooking/bread.md",
            "---\nkp_id: \"kp:ccc\"\nkp_schema: kp-note/v1\ntitle: Bread\n---\n\
             sourdough flour hydration crumb oven steam levain proofing\n"
                .to_owned(),
        ),
    ];
    if with_digest {
        notes.push((
            "digests/2026-07-02.md",
            "---\nkp_id: \"kp:d2\"\nkp_schema: kp-note/v1\ntitle: Digest 2026-07-02\n\
             created: 2026-07-02T06:00:00Z\n---\ndigest of the day\n"
                .to_owned(),
        ));
    }
    for (path, content) in &notes {
        // Notes live in the vault AND the index, like after a real ingest.
        let abs = vault.join(path);
        std::fs::create_dir_all(abs.parent().expect("parent")).expect("mkdir");
        std::fs::write(&abs, content).expect("write note");
        let note = Note::parse(*path, content.as_str()).expect("parses");
        index.upsert_note(&note, &e, params).expect("upsert");
    }
    index
        .add_link("kp:aaa", "curio:bbb", "wikilink")
        .expect("link");
    index.close().expect("close");

    KpConfig::from_toml_str(&format!(
        "schema = \"kp-config/v1\"\n\
         [vault]\npath = \"{}\"\n\
         [index]\npath = \"{}\"\nembedder = \"hash\"\n",
        vault.display(),
        index_path.display(),
    ))
    .expect("config parses")
}

/// Spin up a server over duplex pipes and hand back a connected client.
async fn connect(config: KpConfig) -> RunningService<RoleClient, TestClient> {
    let engine = Arc::new(KpEngine::from_config(config).expect("engine"));
    let (client_w, server_r) = tokio::io::duplex(1 << 16);
    let (server_w, client_r) = tokio::io::duplex(1 << 16);
    tokio::spawn(async move {
        let running = KpMcpServer::new(engine)
            .serve((server_r, server_w))
            .await
            .expect("server handshake");
        let _ = running.waiting().await;
    });
    TestClient
        .serve((client_r, client_w))
        .await
        .expect("client handshake")
}

async fn call(
    client: &RunningService<RoleClient, TestClient>,
    tool: &str,
    args: serde_json::Value,
) -> CallToolResult {
    let mut params = CallToolRequestParams::new(tool.to_owned());
    if let serde_json::Value::Object(map) = args {
        params = params.with_arguments(map);
    }
    client.call_tool(params).await.expect("call_tool transport")
}

fn structured<T: serde::de::DeserializeOwned>(result: &CallToolResult) -> T {
    assert_ne!(
        result.is_error,
        Some(true),
        "tool errored: {:?}",
        result.content
    );
    serde_json::from_value(
        result
            .structured_content
            .clone()
            .expect("structured content"),
    )
    .expect("output matches the documented shape")
}

#[tokio::test]
async fn tools_list_is_exactly_the_v1_contract() {
    let dir = tempfile::tempdir().expect("tempdir");
    let client = connect(seed(dir.path(), false)).await;
    let tools = client.list_all_tools().await.expect("list");
    let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    let mut expected: Vec<String> = TOOLS_V1.iter().map(|s| (*s).to_owned()).collect();
    expected.sort();
    assert_eq!(names, expected);
    client.cancel().await.expect("shutdown");
}

#[tokio::test]
async fn kp_search_serves_all_three_modes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let client = connect(seed(dir.path(), false)).await;

    for mode in ["hybrid", "vector", "fts"] {
        let result = call(
            &client,
            "kp_search",
            json!({"query": "sqlite database", "mode": mode, "k": 2}),
        )
        .await;
        let out: SearchOutput = structured(&result);
        assert!(
            !out.results.is_empty(),
            "{mode} found nothing for a seeded topic"
        );
        assert!(out.results.len() <= 2, "k honored");
        let ids: Vec<&str> = out.results.iter().map(|h| h.id.as_str()).collect();
        assert!(
            ids.contains(&"kp:aaa") || ids.contains(&"curio:bbb"),
            "{mode} missed the database notes: {ids:?}"
        );
        for hit in &out.results {
            assert!(!hit.title.is_empty());
            assert!(!hit.path.is_empty());
        }
    }

    // Defaults: no k, no mode → hybrid, up to 10.
    let result = call(&client, "kp_search", json!({"query": "rust"})).await;
    let out: SearchOutput = structured(&result);
    assert_eq!(out.mode.to_string(), "hybrid");
    assert!(out.results.len() <= 10);
    client.cancel().await.expect("shutdown");
}

#[tokio::test]
async fn kp_get_note_returns_content_frontmatter_and_index_metadata() {
    let dir = tempfile::tempdir().expect("tempdir");
    let client = connect(seed(dir.path(), false)).await;

    let result = call(&client, "kp_get_note", json!({"id": "kp:aaa"})).await;
    let out: NoteOutput = structured(&result);
    assert_eq!(out.id, "kp:aaa");
    assert_eq!(out.title, "Rust databases");
    assert_eq!(out.path, "rust/db.md");
    assert!(out.content.contains("storage engine"));
    assert_eq!(out.frontmatter.tags, vec!["rust", "databases"]);
    assert_eq!(
        out.frontmatter.source.as_deref(),
        Some("https://example.com/rust-db")
    );
    assert!(out.index.ingested_at.ends_with('Z'));
    assert_eq!(out.index.links.len(), 1);
    assert_eq!(out.index.links[0].to, "curio:bbb");

    // Unknown id → a tool error with a readable message, not a crash.
    let result = call(&client, "kp_get_note", json!({"id": "kp:nope"})).await;
    assert_eq!(result.is_error, Some(true));
    let text = result.content[0].as_text().expect("text content");
    assert!(text.text.contains("kp:nope"), "message names the id");
    client.cancel().await.expect("shutdown");
}

#[tokio::test]
async fn kp_related_ranks_the_topical_neighbor_first() {
    let dir = tempfile::tempdir().expect("tempdir");
    let client = connect(seed(dir.path(), false)).await;

    let result = call(&client, "kp_related", json!({"id": "kp:aaa", "k": 2})).await;
    let out: RelatedOutput = structured(&result);
    assert_eq!(out.id, "kp:aaa");
    assert!(!out.results.is_empty());
    assert!(
        out.results.iter().all(|h| h.id != "kp:aaa"),
        "self excluded"
    );
    assert_eq!(out.results[0].id, "curio:bbb", "results: {:?}", out.results);

    let result = call(&client, "kp_related", json!({"id": "kp:nope"})).await;
    assert_eq!(result.is_error, Some(true));
    client.cancel().await.expect("shutdown");
}

#[tokio::test]
async fn kp_recent_lists_and_filters_by_kind() {
    let dir = tempfile::tempdir().expect("tempdir");
    let client = connect(seed(dir.path(), false)).await;

    let result = call(&client, "kp_recent", json!({})).await;
    let out: RecentOutput = structured(&result);
    assert_eq!(out.days, 7, "contract default");
    assert_eq!(out.notes.len(), 3, "everything was just ingested");

    let result = call(&client, "kp_recent", json!({"days": 30, "kind": "curio"})).await;
    let out: RecentOutput = structured(&result);
    assert_eq!(out.days, 30);
    assert_eq!(out.notes.len(), 1);
    assert_eq!(out.notes[0].id, "curio:bbb");
    client.cancel().await.expect("shutdown");
}

#[tokio::test]
async fn kp_propose_writes_only_the_proposals_layout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = seed(dir.path(), false);
    let vault_root = config.vault_path();
    let client = connect(config).await;

    let result = call(
        &client,
        "kp_propose",
        json!({
            "title": "Add an idea note",
            "rationale": "captured during a session",
            "files": [{"path": "ideas/one.md", "content": "# One\n\nbody\n"}]
        }),
    )
    .await;
    let out: ProposeOutput = structured(&result);
    assert_eq!(out.status, "open");
    assert_eq!(out.files, vec!["ideas/one.md"]);
    assert_eq!(out.dir, format!(".kp/proposals/{}", out.id));

    // proposals/v1 layout exists...
    let pdir = vault_root.join(&out.dir);
    assert!(pdir.join("proposal.json").is_file());
    assert!(pdir.join("changes.patch").is_file());
    // ...and the TARGET was not written: proposals are the only write.
    assert!(!vault_root.join("ideas/one.md").exists());

    // Producer-owned state is a hard reject over the wire too.
    let result = call(
        &client,
        "kp_propose",
        json!({
            "title": "evil",
            "rationale": "n/a",
            "files": [{"path": ".curio/manifest.json", "content": "{}"}]
        }),
    )
    .await;
    assert_eq!(result.is_error, Some(true));
    let text = result.content[0].as_text().expect("text content");
    assert!(text.text.contains(".curio"), "message: {}", text.text);
    client.cancel().await.expect("shutdown");
}

#[tokio::test]
async fn kp_digest_latest_is_null_then_the_newest_digest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let client = connect(seed(dir.path(), false)).await;
    let result = call(&client, "kp_digest_latest", json!({})).await;
    let out: DigestOutput = structured(&result);
    assert!(out.digest.is_none(), "no digest seeded yet");
    client.cancel().await.expect("shutdown");

    let dir = tempfile::tempdir().expect("tempdir");
    let client = connect(seed(dir.path(), true)).await;
    let result = call(&client, "kp_digest_latest", json!({})).await;
    let out: DigestOutput = structured(&result);
    let digest = out.digest.expect("digest present");
    assert_eq!(digest.id, "kp:d2");
    assert_eq!(digest.path, "digests/2026-07-02.md");
    assert!(digest.content.contains("digest of the day"));
    client.cancel().await.expect("shutdown");
}
