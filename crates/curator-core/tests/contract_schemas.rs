//! Published-schema conformance: the JSON Schemas under `contracts/` are
//! normative, so what curator-core WRITES must validate against them, and the
//! schemas must still reject nonconforming documents. A failure here is a
//! code bug (contract-first), never a reason to edit a published schema.

use curator_core::{KpId, Note, NoteFrontmatter, ProposalFile, Vault, create_proposal};
use serde_json::{Value, json};

const KP_NOTE_SCHEMA: &str = include_str!("../../../contracts/kp-note/v1.schema.json");
const PROPOSALS_SCHEMA: &str = include_str!("../../../contracts/proposals/v1.schema.json");
const KP_CONFIG_SCHEMA: &str = include_str!("../../../contracts/kp-config/v1.schema.json");
const EXAMPLE_CONFIG: &str = include_str!("../../../curator.example.toml");
const COMPOSE_CONFIG: &str = include_str!("../../../examples/compose/curator.toml");

/// Compile a published schema with format assertion ON (`date-time`,
/// `uri` are normative in the contracts, not annotations).
fn compile(raw: &str) -> jsonschema::Validator {
    let doc: Value = serde_json::from_str(raw).expect("published schema is valid JSON");
    jsonschema::options()
        .should_validate_formats(true)
        .build(&doc)
        .expect("published schema compiles")
}

fn assert_valid(schema: &jsonschema::Validator, instance: &Value) {
    let errors: Vec<String> = schema
        .iter_errors(instance)
        .map(|e| e.to_string())
        .collect();
    assert!(
        errors.is_empty(),
        "expected valid, got: {errors:?}\n{instance:#}"
    );
}

fn assert_invalid(schema: &jsonschema::Validator, instance: &Value, why: &str) {
    assert!(
        !schema.is_valid(instance),
        "schema failed to reject: {why}\n{instance:#}"
    );
}

// ---------------------------------------------------------------- kp-note/v1

/// A fully-enriched note, as a producer would write it.
const ENRICHED_NOTE: &str = "---\n\
kp_id: \"curio:0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d\"\n\
kp_schema: kp-note/v1\n\
checksum: \"sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08\"\n\
title: \"Async patterns\"\n\
created: 2026-07-01T12:00:00Z\n\
updated: 2026-07-03T09:15:00Z\n\
tags: [rust, async]\n\
source: \"https://example.com/async\"\n\
---\nbody\n";

#[test]
fn kp_note_schema_accepts_what_curator_core_parses_and_serializes() {
    let schema = compile(KP_NOTE_SCHEMA);

    // The minimal block curator-core mints.
    let minimal =
        NoteFrontmatter::new(KpId::Kp("0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d".into()), "T");
    assert_valid(
        &schema,
        &serde_json::to_value(&minimal).expect("serializes"),
    );

    // A fully-enriched producer note, round-tripped through Note::parse.
    let note = Note::parse("curio/async.md", ENRICHED_NOTE).expect("parses");
    let curator_core::Frontmatter::Kp(fm) = &note.frontmatter else {
        panic!("KP frontmatter expected");
    };
    assert_valid(&schema, &serde_json::to_value(fm).expect("serializes"));

    // Unknown keys are explicitly allowed (additionalProperties: true).
    let mut extra = serde_json::to_value(&minimal).expect("serializes");
    extra["some_other_tool"] = json!("their metadata");
    assert_valid(&schema, &extra);
}

#[test]
fn kp_note_schema_rejects_nonconforming_frontmatter() {
    let schema = compile(KP_NOTE_SCHEMA);
    let valid = serde_json::to_value(NoteFrontmatter::new(
        KpId::Kp("0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d".into()),
        "T",
    ))
    .expect("serializes");

    let mutations: [(&str, Value); 6] = [
        ("missing kp_id", {
            let mut v = valid.clone();
            v.as_object_mut().expect("object").remove("kp_id");
            v
        }),
        ("unknown kp_id namespace", {
            let mut v = valid.clone();
            v["kp_id"] = json!("unknown:abc");
            v
        }),
        ("wrong kp_schema pin", {
            let mut v = valid.clone();
            v["kp_schema"] = json!("kp-note/v2");
            v
        }),
        ("checksum is a change token: sha256 hex only", {
            let mut v = valid.clone();
            v["checksum"] = json!("md5:abcd");
            v
        }),
        ("empty title", {
            let mut v = valid.clone();
            v["title"] = json!("");
            v
        }),
        // `status` is reserved AGAINST (schema: `"status": false`) — the
        // plane must never grow workflow state inside note frontmatter.
        ("reserved status key", {
            let mut v = valid.clone();
            v["status"] = json!("draft");
            v
        }),
    ];
    for (why, instance) in &mutations {
        assert_invalid(&schema, instance, why);
    }
}

#[test]
fn kp_note_schema_asserts_timestamp_and_uri_formats() {
    let schema = compile(KP_NOTE_SCHEMA);
    let mut v = serde_json::to_value(NoteFrontmatter::new(
        KpId::Kp("0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d".into()),
        "T",
    ))
    .expect("serializes");
    v["created"] = json!("yesterday-ish");
    assert_invalid(&schema, &v, "created must be RFC 3339");
}

// --------------------------------------------------------------- proposals/v1

#[test]
fn proposals_schema_accepts_what_create_proposal_writes() {
    let schema = compile(PROPOSALS_SCHEMA);
    let dir = tempfile::tempdir().expect("tempdir");
    let vault = Vault::open(dir.path()).expect("vault opens");
    let proposal = create_proposal(
        &vault,
        ".kp/proposals",
        "kp-librarian",
        "Add a note",
        "conformance test",
        &[ProposalFile {
            path: "notes/new.md".to_owned(),
            content: "hello\n".to_owned(),
        }],
    )
    .expect("proposal writes");

    // Validate the ACTUAL on-disk proposal.json, not the in-memory struct.
    let raw = vault
        .read(&format!(".kp/proposals/{}/proposal.json", proposal.id))
        .expect("proposal.json exists");
    let on_disk: Value = serde_json::from_str(&raw).expect("proposal.json is JSON");
    assert_valid(&schema, &on_disk);
}

#[test]
fn proposals_schema_rejects_nonconforming_documents() {
    let schema = compile(PROPOSALS_SCHEMA);
    let valid = json!({
        "schema": "proposals/v1",
        "id": "01JZ8Y0Q4R5S6T7V8W9X0Y1Z2A",
        "created": "2026-07-03T09:15:00Z",
        "author": "kp-librarian",
        "title": "Add a note",
        "rationale": "why",
        "status": "open",
        "files": ["notes/new.md"]
    });
    assert_valid(&schema, &valid);

    let mutations: [(&str, Value); 5] = [
        ("status outside the lifecycle enum", {
            let mut v = valid.clone();
            v["status"] = json!("merged");
            v
        }),
        ("id is not a ULID", {
            let mut v = valid.clone();
            v["id"] = json!("not-a-ulid");
            v
        }),
        ("empty files array", {
            let mut v = valid.clone();
            v["files"] = json!([]);
            v
        }),
        ("absolute file path", {
            let mut v = valid.clone();
            v["files"] = json!(["/etc/passwd"]);
            v
        }),
        ("missing rationale", {
            let mut v = valid.clone();
            v.as_object_mut().expect("object").remove("rationale");
            v
        }),
    ];
    for (why, instance) in &mutations {
        assert_invalid(&schema, instance, why);
    }
}

// --------------------------------------------------------------- kp-config/v1

/// Parse a TOML config document to a JSON value, preserving EVERY key (unlike
/// round-tripping through `KpConfig`, which drops unknown keys and applies
/// defaults). This is what makes "the shipped example validates" a real check.
fn toml_to_json(raw: &str) -> Value {
    let doc: toml::Value = toml::from_str(raw).expect("config toml parses");
    serde_json::to_value(doc).expect("toml → json")
}

#[test]
fn kp_config_schema_accepts_the_shipped_example_configs() {
    let schema = compile(KP_CONFIG_SCHEMA);
    // The canonical example a user copies to curator.toml.
    assert_valid(&schema, &toml_to_json(EXAMPLE_CONFIG));
    // The container/compose profile config (http transport, builtin embedder).
    assert_valid(&schema, &toml_to_json(COMPOSE_CONFIG));
}

#[test]
fn kp_config_schema_rejects_nonconforming_documents() {
    let schema = compile(KP_CONFIG_SCHEMA);
    let valid = toml_to_json(EXAMPLE_CONFIG);
    assert_valid(&schema, &valid);

    let mutations: [(&str, Value); 4] = [
        ("wrong schema pin", {
            let mut v = valid.clone();
            v["schema"] = json!("kp-config/v2");
            v
        }),
        ("embedder outside the normative value set", {
            let mut v = valid.clone();
            v["index"]["embedder"] = json!("gpt-magic");
            v
        }),
        ("mcp transport outside the normative value set", {
            let mut v = valid.clone();
            v["mcp"]["transport"] = json!("grpc");
            v
        }),
        ("api_base is not a URI", {
            let mut v = valid.clone();
            v["zotero"]["api_base"] = json!("not a url");
            v
        }),
    ];
    for (why, instance) in &mutations {
        assert_invalid(&schema, instance, why);
    }
}

#[test]
fn kp_config_schema_tolerates_forward_compatible_unknown_keys() {
    // The loader warns-not-fails on unknown keys; the schema must agree
    // (additionalProperties: true), or the contract's forward-compatibility
    // promise would be a lie.
    let schema = compile(KP_CONFIG_SCHEMA);
    let mut v = toml_to_json(EXAMPLE_CONFIG);
    v["some_future_table"] = json!({ "k": "v" });
    v["index"]["some_future_key"] = json!(true);
    assert_valid(&schema, &v);
}
