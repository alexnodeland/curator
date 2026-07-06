//! The vendored-schema PIN is ENFORCED, not just recorded: this test
//! recomputes the sha256 of the compile-time-embedded vendored Curio
//! schemas and checks them against `contracts/vendor/curio/PIN`. A local
//! edit to a vendored file (which would silently change the boundary
//! validator while PIN still attests the old bytes) fails the suite —
//! vendored copies are byte-identical to the recorded upstream sync, or
//! they are re-synced with a new PIN in their own reviewed commit.

use curator_core::Checksum;

const PIN: &str = include_str!("../../../contracts/vendor/curio/PIN");

/// The `sha256 <file>:` line of PIN, parsed.
fn pinned_sha(file: &str) -> String {
    let key = format!("sha256 {file}:");
    PIN.lines()
        .find_map(|line| line.strip_prefix(&key))
        .unwrap_or_else(|| panic!("PIN has no `{key}` line:\n{PIN}"))
        .trim()
        .to_owned()
}

#[test]
fn vendored_schemas_match_the_pin() {
    for (file, embedded) in [
        (
            "frontmatter.v1.json",
            include_str!("../../../contracts/vendor/curio/frontmatter.v1.json"),
        ),
        (
            "events.v1.json",
            include_str!("../../../contracts/vendor/curio/events.v1.json"),
        ),
    ] {
        let got = Checksum::compute(embedded.as_bytes());
        assert_eq!(
            got.hex(),
            pinned_sha(file),
            "{file} does not match contracts/vendor/curio/PIN — vendored \
             schemas are never edited in place; re-sync from upstream and \
             record a new PIN in its own commit"
        );
    }
}

#[test]
fn pin_carries_full_provenance() {
    for key in ["source_repo:", "commit:", "synced:"] {
        assert!(
            PIN.lines().any(|l| l.starts_with(key)),
            "PIN must record {key}"
        );
    }
}
