//! End-to-end librarian flows through the `curator` binary: ingest → digest
//! run --auto (idempotent by date), the propose/review/apply lifecycle,
//! doctor, and init. Hermetic: hash embedder, temp dirs, injected clock.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// 2026-07-03T09:15:00Z.
const NOW: &str = "2026-07-03T09:15:00Z";

fn curator_bin() -> &'static str {
    env!("CARGO_BIN_EXE_curator")
}

/// Seed a vault of FILES (no pre-built index) + a hash-embedder curator.toml.
fn seed_vault(dir: &Path) -> PathBuf {
    let vault = dir.join("vault");
    std::fs::create_dir_all(&vault).expect("mkdir vault");
    let notes: &[(&str, &str)] = &[
        (
            "now.md",
            "# Now\n\nrust database sqlite embedded storage engine work\n",
        ),
        (
            "rust/db.md",
            "---\nkp_id: \"kp:0197aaaa-0000-7000-8000-000000000001\"\nkp_schema: kp-note/v1\n\
             title: Rust databases\ntags: [rust]\ncreated: 2026-07-01T00:00:00Z\n---\n\
             Embedded sqlite storage engine notes for rust database work.\n",
        ),
        (
            "cooking/bread.md",
            "---\nkp_id: \"kp:0197aaaa-0000-7000-8000-000000000003\"\nkp_schema: kp-note/v1\n\
             title: Bread\ntags: [cooking]\ncreated: 2026-07-02T00:00:00Z\n---\n\
             Sourdough hydration and proofing schedule.\n",
        ),
    ];
    for (path, content) in notes {
        let abs = vault.join(path);
        std::fs::create_dir_all(abs.parent().expect("parent")).expect("mkdir");
        std::fs::write(abs, content).expect("write note");
    }
    let config_path = dir.join("curator.toml");
    std::fs::write(
        &config_path,
        format!(
            "schema = \"kp-config/v1\"\n\
             [vault]\npath = \"{}\"\n\
             [index]\npath = \"{}\"\nembedder = \"hash\"\n",
            vault.display(),
            dir.join("index.db").display(),
        ),
    )
    .expect("write config");
    config_path
}

fn curator(config: &Path, args: &[&str]) -> Output {
    Command::new(curator_bin())
        .args(args)
        .arg("--config")
        .arg(config)
        .output()
        .expect("curator runs")
}

fn ok_json(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "curator failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("stdout is JSON")
}

#[test]
fn digest_run_auto_is_idempotent_by_date() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = seed_vault(dir.path());
    let vault = dir.path().join("vault");

    assert!(curator(&config, &["ingest"]).status.success());
    let out = ok_json(&curator(
        &config,
        &["digest", "run", "--auto", "--now", NOW, "--json"],
    ));
    assert_eq!(out["date"], "2026-07-03");
    assert_eq!(out["applied"], true);
    assert_eq!(out["skipped"], serde_json::Value::Null);
    assert_eq!(out["candidates"], 2, "now.md excluded as the anchor");

    // The digest note landed in the vault via the proposal.
    let digest_path = vault.join("digests/2026-07-03.md");
    let content = std::fs::read_to_string(&digest_path).expect("digest exists");
    assert!(content.contains("# Daily digest 2026-07-03"), "{content}");
    assert!(content.contains("kp_id: kp:"), "{content}");
    assert!(content.contains("[[rust/db|Rust databases]]"), "{content}");

    // Second run with the SAME clock: a no-op — no duplicate note, no
    // duplicate proposal.
    let out = ok_json(&curator(
        &config,
        &["digest", "run", "--auto", "--now", NOW, "--json"],
    ));
    assert_ne!(out["skipped"], serde_json::Value::Null, "{out}");
    let proposals = ok_json(&curator(&config, &["proposals", "list", "--json"]));
    let proposals = proposals.as_array().expect("array");
    assert_eq!(proposals.len(), 1, "exactly one proposal");
    assert_eq!(proposals[0]["status"], "applied");
    assert_eq!(proposals[0]["author"], "kp-librarian");
    assert_eq!(
        std::fs::read_to_string(&digest_path).expect("read"),
        content,
        "digest note untouched"
    );
}

#[test]
fn applied_digest_is_served_by_the_read_surface_immediately() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = seed_vault(dir.path());
    assert!(curator(&config, &["ingest"]).status.success());
    let report = ok_json(&curator(
        &config,
        &["digest", "run", "--auto", "--now", NOW, "--json"],
    ));
    let kp_id = report["kp_id"].as_str().expect("kp_id");
    // No re-ingest between digest and get: the applied digest is indexed.
    let note = ok_json(&curator(&config, &["get", kp_id, "--json"]));
    assert_eq!(note["title"], "Daily digest 2026-07-03");
    assert_eq!(note["path"], "digests/2026-07-03.md");
    assert_eq!(note["frontmatter"]["tags"][0], "digest");
}

#[test]
fn propose_review_apply_round_trip_and_reject_on_drift() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = seed_vault(dir.path());
    let vault = dir.path().join("vault");
    assert!(curator(&config, &["ingest"]).status.success());

    // Stage a generated changeset: one new note, one edit.
    let stage = dir.path().join("stage");
    std::fs::create_dir_all(stage.join("notes")).expect("mkdir");
    std::fs::write(stage.join("notes/idea.md"), "# Idea\n\nfrom the stage\n").expect("write");

    let out = ok_json(&curator(
        &config,
        &[
            "propose",
            "--title",
            "Add an idea",
            "--rationale",
            "testing",
            "--from",
            stage.to_str().expect("utf8"),
            "--json",
        ],
    ));
    let id = out["id"].as_str().expect("id").to_owned();
    assert_eq!(out["status"], "open");
    assert_eq!(out["files"][0], "notes/idea.md");

    // Review renders metadata + hunks.
    let review = curator(&config, &["review", &id]);
    assert!(review.status.success());
    let text = String::from_utf8_lossy(&review.stdout);
    assert!(text.contains("Add an idea"), "{text}");
    assert!(text.contains("+# Idea"), "{text}");

    // Apply writes the file and stamps the status.
    let out = ok_json(&curator(&config, &["apply", &id, "--json"]));
    assert_eq!(out["status"], "applied");
    assert!(vault.join("notes/idea.md").exists());

    // A drifted proposal is rejected and stamped rejected.
    std::fs::write(stage.join("notes/idea.md"), "# Idea\n\nrewritten v2\n").expect("write");
    let out = ok_json(&curator(
        &config,
        &[
            "propose",
            "--title",
            "Edit the idea",
            "--from",
            stage.to_str().expect("utf8"),
            "--json",
        ],
    ));
    let id2 = out["id"].as_str().expect("id").to_owned();
    // The vault moves on underneath the proposal.
    std::fs::write(
        vault.join("notes/idea.md"),
        "# Idea\n\nhand-edited meanwhile\n",
    )
    .expect("drift");
    let apply = curator(&config, &["apply", &id2]);
    assert!(!apply.status.success(), "drifted apply must fail");
    let stderr = String::from_utf8_lossy(&apply.stderr);
    assert!(stderr.contains("rejected"), "{stderr}");
    let proposals = ok_json(&curator(&config, &["proposals", "list", "--json"]));
    let statuses: Vec<(&str, &str)> = proposals
        .as_array()
        .expect("array")
        .iter()
        .map(|p| {
            (
                p["id"].as_str().expect("id"),
                p["status"].as_str().expect("status"),
            )
        })
        .collect();
    assert!(statuses.contains(&(id.as_str(), "applied")), "{statuses:?}");
    assert!(
        statuses.contains(&(id2.as_str(), "rejected")),
        "{statuses:?}"
    );
}

#[test]
fn doctor_reports_health_before_and_after_ingest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = seed_vault(dir.path());

    // Pre-ingest: missing index is a warning, not a failure.
    let out = curator(&config, &["doctor", "--json"]);
    let checks = ok_json(&out);
    let level_of = |checks: &serde_json::Value, name: &str| {
        checks
            .as_array()
            .expect("array")
            .iter()
            .find(|c| c["check"] == name)
            .unwrap_or_else(|| panic!("no {name} check in {checks}"))["level"]
            .as_str()
            .expect("level")
            .to_owned()
    };
    assert_eq!(level_of(&checks, "vault"), "ok");
    assert_eq!(level_of(&checks, "index"), "warn");
    assert_eq!(level_of(&checks, "now.md"), "ok");

    assert!(curator(&config, &["ingest"]).status.success());
    let checks = ok_json(&curator(&config, &["doctor", "--json"]));
    assert_eq!(level_of(&checks, "index"), "ok");
    assert_eq!(level_of(&checks, "embedder"), "ok");
    assert_eq!(level_of(&checks, "mcp"), "ok");

    // An embedder mismatch is an error and fails the run.
    let toml = std::fs::read_to_string(&config).expect("read config");
    std::fs::write(
        &config,
        toml.replace("embedder = \"hash\"", "embedder = \"builtin\""),
    )
    .expect("write");
    let out = curator(&config, &["doctor", "--json"]);
    assert!(!out.status.success(), "embedder mismatch must fail doctor");
    let checks: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(level_of(&checks, "embedder"), "error");
}

#[test]
fn init_scaffolds_a_working_setup() {
    let dir = tempfile::tempdir().expect("tempdir");
    let vault = dir.path().join("fresh");
    std::fs::create_dir_all(vault.join("notes")).expect("mkdir");
    std::fs::write(
        vault.join("notes/first.md"),
        "# First\n\nsqlite database notes\n",
    )
    .expect("write");

    let out = Command::new(curator_bin())
        .args(["init", vault.to_str().expect("utf8"), "--embedder", "hash"])
        .output()
        .expect("curator runs");
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let config = vault.join("curator.toml");
    assert!(config.exists());
    assert!(vault.join(".kp/proposals").is_dir());
    assert!(vault.join("now.md").exists());
    assert!(vault.join(".kp/index.db").exists(), "first index built");
    let toml = std::fs::read_to_string(&config).expect("read");
    assert!(toml.contains("embedder = \"hash\""), "{toml}");

    // Idempotent: a second init changes nothing and still succeeds.
    let before = std::fs::read_to_string(&config).expect("read");
    let out = Command::new(curator_bin())
        .args(["init", vault.to_str().expect("utf8"), "--embedder", "hash"])
        .output()
        .expect("curator runs");
    assert!(out.status.success());
    assert_eq!(std::fs::read_to_string(&config).expect("read"), before);

    // The scaffolded setup actually serves queries.
    let search = Command::new(curator_bin())
        .args(["search", "sqlite", "--json", "--config"])
        .arg(&config)
        .output()
        .expect("curator runs");
    assert!(search.status.success());
    let out: serde_json::Value = serde_json::from_slice(&search.stdout).expect("json");
    assert!(
        !out["results"].as_array().expect("results").is_empty(),
        "{out}"
    );
}
