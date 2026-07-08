//! End-to-end tests of the `curator` binary: the query commands (which share
//! curator-mcp's engine, so these cover the MCP tool logic too), the stdio
//! MCP framing, and the http-mode startup refusal.
//!
//! Hermetic: hash embedder, temp vaults, no network — the only processes
//! spawned are the compiled `curator` binary itself.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use curator_core::note::Note;
use curator_index::{ChunkParams, HashEmbedder, Index};

fn curator_bin() -> &'static str {
    env!("CARGO_BIN_EXE_curator")
}

/// Seed a vault + hash-embedded index; returns the curator.toml path.
fn seed(dir: &Path) -> PathBuf {
    let vault = dir.join("vault");
    std::fs::create_dir_all(&vault).expect("mkdir vault");
    let index_path = dir.join("index.db");

    let e = HashEmbedder::default();
    let mut index = Index::create(&index_path, &e, 1).expect("create index");
    let params = ChunkParams {
        tokens: 16,
        overlap: 2,
    };
    for (path, content) in [
        (
            "rust/db.md",
            "---\nkp_id: \"kp:aaa\"\nkp_schema: kp-note/v1\ntitle: Rust databases\n\
             tags: [rust, databases]\n---\n\
             rust database embedded sqlite storage engine queries indexes design\n",
        ),
        (
            "rust/async.md",
            "---\nkp_id: \"curio:bbb\"\nkp_schema: kp-note/v1\ntitle: Async rust\n---\n\
             rust database embedded sqlite storage engine queries indexes async\n",
        ),
        (
            "cooking/bread.md",
            "---\nkp_id: \"kp:ccc\"\nkp_schema: kp-note/v1\ntitle: Bread\n---\n\
             sourdough flour hydration crumb oven steam levain proofing\n",
        ),
    ] {
        let abs = vault.join(path);
        std::fs::create_dir_all(abs.parent().expect("parent")).expect("mkdir");
        std::fs::write(&abs, content).expect("write note");
        let note = Note::parse(path, content).expect("parses");
        index.upsert_note(&note, &e, params).expect("upsert");
    }
    index.close().expect("close");

    let config_path = dir.join("curator.toml");
    std::fs::write(
        &config_path,
        format!(
            "schema = \"kp-config/v1\"\n\
             [vault]\npath = \"{}\"\n\
             [index]\npath = \"{}\"\nembedder = \"hash\"\n",
            vault.display(),
            index_path.display(),
        ),
    )
    .expect("write config");
    config_path
}

fn curator(config: &Path, args: &[&str]) -> Output {
    Command::new(curator_bin())
        .arg(args[0])
        .args(&args[1..])
        .arg("--config")
        .arg(config)
        .output()
        .expect("curator runs")
}

fn stdout_json(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "curator failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("stdout is JSON")
}

#[test]
fn search_finds_seeded_notes_in_every_mode() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = seed(dir.path());

    for mode in ["hybrid", "vector", "fts"] {
        let out = stdout_json(&curator(
            &config,
            &[
                "search",
                "sqlite database",
                "--mode",
                mode,
                "--k",
                "2",
                "--json",
            ],
        ));
        assert_eq!(out["mode"], mode);
        let results = out["results"].as_array().expect("results array");
        assert!(!results.is_empty(), "{mode} found nothing");
        assert!(results.len() <= 2);
        for hit in results {
            for key in ["id", "title", "path", "snippet", "score"] {
                assert!(hit.get(key).is_some(), "{mode} hit missing {key}");
            }
        }
    }

    // Human output mentions the hit.
    let out = curator(&config, &["search", "sqlite"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Rust databases"), "got: {text}");
}

#[test]
fn get_returns_the_full_note_and_fails_on_unknown_ids() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = seed(dir.path());

    let out = stdout_json(&curator(&config, &["get", "kp:aaa", "--json"]));
    assert_eq!(out["id"], "kp:aaa");
    assert_eq!(out["title"], "Rust databases");
    assert_eq!(out["path"], "rust/db.md");
    assert_eq!(out["frontmatter"]["tags"][0], "rust");
    assert!(
        out["content"]
            .as_str()
            .expect("content")
            .contains("storage engine")
    );
    assert!(out["index"]["ingested_at"].as_str().is_some());

    let out = curator(&config, &["get", "kp:nope", "--json"]);
    assert!(!out.status.success(), "unknown id must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("kp:nope"), "stderr names the id: {stderr}");
}

#[test]
fn related_ranks_the_topical_neighbor_first() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = seed(dir.path());
    let out = stdout_json(&curator(
        &config,
        &["related", "kp:aaa", "--k", "2", "--json"],
    ));
    assert_eq!(out["id"], "kp:aaa");
    let results = out["results"].as_array().expect("results");
    assert!(!results.is_empty());
    assert_eq!(results[0]["id"], "curio:bbb");
    assert!(results.iter().all(|h| h["id"] != "kp:aaa"), "self excluded");
}

#[test]
fn recent_lists_and_filters_by_kind() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = seed(dir.path());

    let out = stdout_json(&curator(&config, &["recent", "--json"]));
    assert_eq!(out["days"], 7);
    assert_eq!(out["notes"].as_array().expect("notes").len(), 3);

    let out = stdout_json(&curator(&config, &["recent", "--kind", "curio", "--json"]));
    let notes = out["notes"].as_array().expect("notes");
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["id"], "curio:bbb");
}

/// A scripted MCP session over stdio: initialize → initialized →
/// tools/list → one tools/call, framed as newline-delimited JSON-RPC —
/// exactly what any MCP client does with `command: curator, args: [mcp, serve]`.
#[test]
fn mcp_serve_speaks_mcp_over_stdio() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = seed(dir.path());

    let mut child = Command::new(curator_bin())
        .args(["mcp", "serve", "--config"])
        .arg(&config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("curator mcp serve spawns");

    let mut stdin = child.stdin.take().expect("stdin");
    let script = [
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "smoke", "version": "0"}}}),
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
            "name": "kp_search", "arguments": {"query": "sqlite", "k": 1}}}),
    ];
    for message in &script {
        writeln!(stdin, "{message}").expect("write frame");
    }
    drop(stdin); // EOF → clean server shutdown

    let output = child.wait_with_output().expect("curator exits");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let frames: Vec<serde_json::Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("every stdout line is JSON-RPC"))
        .collect();
    let by_id = |id: u64| {
        frames
            .iter()
            .find(|f| f["id"] == id)
            .unwrap_or_else(|| panic!("no response with id {id}: {frames:?}"))
    };

    let init = by_id(1);
    assert_eq!(init["result"]["serverInfo"]["name"], "rmcp");
    let tools = by_id(2)["result"]["tools"].as_array().expect("tools array");
    let mut names: Vec<&str> = tools
        .iter()
        .map(|t| t["name"].as_str().expect("name"))
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "kp_digest_latest",
            "kp_get_note",
            "kp_propose",
            "kp_recent",
            "kp_related",
            "kp_search"
        ],
        "the six v1 tools, exactly"
    );
    let call = by_id(3);
    assert_ne!(call["result"]["isError"], true);
    let results = call["result"]["structuredContent"]["results"]
        .as_array()
        .expect("structured results");
    assert!(!results.is_empty(), "kp_search found the seeded note");
}

/// Contract binding rule 4: http transport with no bearer token in the
/// environment refuses to start.
#[test]
fn mcp_serve_http_refuses_without_a_bearer_token() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = seed(dir.path());

    let output = Command::new(curator_bin())
        .args(["mcp", "serve", "--http", "--config"])
        .arg(&config)
        .env_remove("KP_MCP_TOKEN")
        .env_remove("CURATOR_MCP_TOKEN")
        .output()
        .expect("curator runs");
    assert!(!output.status.success(), "must refuse to start");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("KP_MCP_TOKEN") && stderr.contains("refusing to start"),
        "stderr: {stderr}"
    );
}

/// kp-config/v1 binding rule 2: "unknown keys warn, never fail". The
/// warning must be USER-OBSERVABLE — the binary installs a tracing
/// subscriber, so a typoed section lands on stderr instead of being
/// silently dropped (a misspelled `[vualt]` means the whole table falls
/// back to defaults; silence is exactly the failure the rule exists to
/// surface).
#[test]
fn unknown_config_keys_warn_on_stderr_and_do_not_fail() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = seed(dir.path());
    let mut raw = std::fs::read_to_string(&config).expect("read config");
    raw.push_str("\n[vualt]\npath = \"typo\"\n\n[index2]\nchunk_token = 3\n");
    std::fs::write(&config, raw).expect("write config");

    let out = curator(&config, &["search", "sqlite", "--json"]);
    assert!(
        out.status.success(),
        "unknown keys must never fail: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown config key") && stderr.contains("vualt"),
        "the promised warning must reach stderr, got: {stderr}"
    );
}

/// Config discovery without --config: ./curator.toml is preferred,
/// ./kp.toml stays accepted (with a deprecation note on stderr), and the
/// $CURATOR_CONFIG / $KP_CONFIG variables are honored in that order.
#[test]
fn config_discovery_prefers_curator_toml_and_accepts_legacy_kp_toml() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = seed(dir.path()); // writes ./curator.toml

    let run_in_dir = |cwd: &Path| {
        Command::new(curator_bin())
            .args(["recent", "--json"])
            .current_dir(cwd)
            .env_remove("CURATOR_CONFIG")
            .env_remove("KP_CONFIG")
            .output()
            .expect("curator runs")
    };

    // Preferred spelling: found, no deprecation chatter.
    let out = run_in_dir(dir.path());
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("legacy"),
        "curator.toml must not warn"
    );

    // Legacy spelling alone: accepted, and the deprecation note reaches
    // stderr (kp.toml is never rejected within v1).
    std::fs::rename(&config, dir.path().join("kp.toml")).expect("rename to legacy");
    let out = run_in_dir(dir.path());
    assert!(
        out.status.success(),
        "kp.toml must still work: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("legacy") && stderr.contains("curator.toml"),
        "deprecation note must reach stderr, got: {stderr}"
    );

    // Both present: curator.toml wins. Point the curator.toml at a valid
    // config and the kp.toml at garbage — success proves precedence.
    std::fs::copy(dir.path().join("kp.toml"), dir.path().join("curator.toml")).expect("copy back");
    std::fs::write(dir.path().join("kp.toml"), "schema = \"nonsense/v9\"\n")
        .expect("poison legacy");
    let out = run_in_dir(dir.path());
    assert!(
        out.status.success(),
        "curator.toml must win over kp.toml: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Env-var names: $KP_CONFIG keeps working, $CURATOR_CONFIG is
    // preferred when both are set.
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let good = dir.path().join("curator.toml");
    let out = Command::new(curator_bin())
        .args(["recent", "--json"])
        .current_dir(elsewhere.path())
        .env_remove("CURATOR_CONFIG")
        .env("KP_CONFIG", &good)
        .output()
        .expect("curator runs");
    assert!(
        out.status.success(),
        "$KP_CONFIG must keep working: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = Command::new(curator_bin())
        .args(["recent", "--json"])
        .current_dir(elsewhere.path())
        .env("CURATOR_CONFIG", &good)
        .env("KP_CONFIG", "/nonexistent/kp.toml")
        .output()
        .expect("curator runs");
    assert!(
        out.status.success(),
        "$CURATOR_CONFIG must be preferred: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `curator status` is a state snapshot: it reports the vault note count, the
/// serving index epoch/embedder, and the (empty) proposal queue, in both JSON
/// and human form, and always succeeds.
#[test]
fn status_snapshots_vault_index_and_proposals() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = seed(dir.path()); // 3 notes, hash-embedded index, no proposals

    let out = stdout_json(&curator(&config, &["status", "--json"]));
    assert_eq!(out["vault"]["notes"], 3);
    assert_eq!(out["index"]["notes"], 3);
    assert_eq!(out["index"]["embedder_id"], "hash");
    assert_eq!(out["index"]["epoch"], 1);
    assert!(out["index"]["latest_digest"].is_null(), "no digest run yet");
    assert_eq!(out["proposals"]["total"], 0);
    assert_eq!(out["proposals"]["open"], 0);
    assert_eq!(out["mcp_transport"], "stdio");

    // Human output names the note count and exits 0.
    let out = curator(&config, &["status"]);
    assert!(out.status.success(), "status must never fail");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("3 note(s)"), "got: {text}");
}

/// With no index built yet, `status` reports "not built" rather than failing —
/// it is a snapshot, not a health gate.
#[test]
fn status_reports_a_missing_index_without_failing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let vault = dir.path().join("vault");
    std::fs::create_dir_all(&vault).expect("mkdir vault");
    let config = dir.path().join("curator.toml");
    std::fs::write(
        &config,
        format!(
            "schema = \"kp-config/v1\"\n\
             [vault]\npath = \"{}\"\n\
             [index]\npath = \"{}\"\nembedder = \"hash\"\n",
            vault.display(),
            dir.path().join("index.db").display(),
        ),
    )
    .expect("write config");

    let out = stdout_json(&curator(&config, &["status", "--json"]));
    assert!(out["index"].is_null(), "no index.db yet");
    assert_eq!(out["vault"]["notes"], 0);

    let out = curator(&config, &["status"]);
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("not built"),
        "human output should say the index is not built"
    );
}
