//! The coverage gate.
//!
//! Consumes a `cargo llvm-cov` JSON export (`llvm.coverage.json.export`)
//! and enforces a minimum REGION coverage on the crates where thin tests
//! would silently rot the contracts: `kp-core`, `kp-index`,
//! `kp-librarian`. Every other crate is reported but never gated — the
//! CLI and transport shells earn their coverage through the e2e suites at
//! whatever level honesty produces.
//!
//! The fix for a failing gate is MORE TESTS, never exclusion games: no
//! per-file ignores, no `#[cfg(coverage)]`, no carve-outs. If a region
//! genuinely cannot execute in the hermetic suite (e.g. the opt-in ONNX
//! path), it stays visible in the report and the crate absorbs it.

use std::collections::BTreeMap;
use std::process::ExitCode;

/// Region-coverage floor, in percent, for the gated crates.
const FAIL_UNDER_REGIONS: f64 = 80.0;

/// Crates the floor applies to. Everything else is report-only.
const GATED_CRATES: [&str; 3] = ["kp-core", "kp-index", "kp-librarian"];

/// Regions summed per crate.
#[derive(Debug, Default, Clone, Copy)]
struct Regions {
    count: u64,
    covered: u64,
}

impl Regions {
    fn percent(self) -> f64 {
        if self.count == 0 {
            // An empty crate covers everything it has.
            100.0
        } else {
            self.covered as f64 * 100.0 / self.count as f64
        }
    }
}

/// Run the gate against an export file written by
/// `cargo llvm-cov report --json --summary-only --output-path <path>`.
pub fn run(json_path: Option<&str>) -> ExitCode {
    let Some(json_path) = json_path else {
        eprintln!("coverage-gate: missing <coverage.json> argument");
        return ExitCode::from(2);
    };
    let raw = match std::fs::read_to_string(json_path) {
        Ok(raw) => raw,
        Err(e) => {
            eprintln!("coverage-gate: cannot read {json_path}: {e}");
            return ExitCode::from(2);
        }
    };
    match evaluate(&raw) {
        Ok(report) => {
            print!("{report}");
            if report.failures.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("coverage-gate: {e}");
            ExitCode::from(2)
        }
    }
}

/// The whole gate outcome: per-crate rows plus the failures.
#[derive(Debug)]
struct Report {
    /// crate name -> summed regions, sorted by name.
    per_crate: BTreeMap<String, Regions>,
    /// Human-readable gate violations (empty = pass).
    failures: Vec<String>,
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "coverage gate — regions, fail-under {FAIL_UNDER_REGIONS}% on gated crates"
        )?;
        for (name, regions) in &self.per_crate {
            let mode = if is_gated(name) { "GATE" } else { "info" };
            writeln!(
                f,
                "  [{mode}] {name:<14} {:>6.2}%  ({}/{} regions)",
                regions.percent(),
                regions.covered,
                regions.count
            )?;
        }
        for failure in &self.failures {
            writeln!(f, "  FAIL: {failure}")?;
        }
        if self.failures.is_empty() {
            writeln!(f, "  gate: PASS")?;
        }
        Ok(())
    }
}

fn is_gated(name: &str) -> bool {
    GATED_CRATES.contains(&name)
}

/// Parse the export JSON and apply the gate. Pure — the testable core.
fn evaluate(raw: &str) -> Result<Report, String> {
    let doc: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("invalid JSON: {e}"))?;
    if doc["type"] != "llvm.coverage.json.export" {
        return Err(format!(
            "not an llvm-cov JSON export (type = {})",
            doc["type"]
        ));
    }
    let files = doc["data"][0]["files"]
        .as_array()
        .ok_or("no data[0].files array — was this written with --summary-only?")?;

    let mut per_crate: BTreeMap<String, Regions> = BTreeMap::new();
    for file in files {
        let filename = file["filename"]
            .as_str()
            .ok_or("file entry without a filename")?;
        let Some(krate) = crate_of(filename) else {
            continue; // out-of-workspace source (e.g. registry deps)
        };
        let regions = &file["summary"]["regions"];
        let (Some(count), Some(covered)) = (regions["count"].as_u64(), regions["covered"].as_u64())
        else {
            return Err(format!("{filename}: no summary.regions counts"));
        };
        let entry = per_crate.entry(krate.to_owned()).or_default();
        entry.count += count;
        entry.covered += covered;
    }

    let mut failures = Vec::new();
    for gated in GATED_CRATES {
        match per_crate.get(gated) {
            None => failures.push(format!(
                "{gated}: no coverage data in the export — the gate cannot see it"
            )),
            Some(regions) if regions.percent() < FAIL_UNDER_REGIONS => failures.push(format!(
                "{gated}: {:.2}% region coverage < {FAIL_UNDER_REGIONS}% — write the missing tests",
                regions.percent()
            )),
            Some(_) => {}
        }
    }
    Ok(Report {
        per_crate,
        failures,
    })
}

/// Map a source filename from the export onto its workspace crate.
fn crate_of(filename: &str) -> Option<&str> {
    if let Some(idx) = filename.find("/crates/") {
        return filename[idx + "/crates/".len()..].split('/').next();
    }
    if filename.contains("/xtask/src/") {
        return Some("xtask");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn export(files: &[(&str, u64, u64)]) -> String {
        let files: Vec<serde_json::Value> = files
            .iter()
            .map(|(name, count, covered)| {
                serde_json::json!({
                    "filename": format!("/ws/crates/{name}"),
                    "summary": {"regions": {"count": count, "covered": covered}}
                })
            })
            .collect();
        serde_json::json!({
            "type": "llvm.coverage.json.export",
            "data": [{"files": files}]
        })
        .to_string()
    }

    /// Enough data for every gated crate to pass.
    fn all_green() -> Vec<(&'static str, u64, u64)> {
        vec![
            ("kp-core/src/a.rs", 100, 95),
            ("kp-index/src/b.rs", 100, 90),
            ("kp-librarian/src/c.rs", 100, 85),
        ]
    }

    #[test]
    fn passes_when_every_gated_crate_clears_the_floor() {
        let report = evaluate(&export(&all_green())).expect("evaluates");
        assert!(report.failures.is_empty(), "{:?}", report.failures);
    }

    #[test]
    fn fails_a_gated_crate_under_the_floor() {
        let mut files = all_green();
        files.push(("kp-core/src/cold.rs", 100, 0)); // drags kp-core to 47.5%
        let report = evaluate(&export(&files)).expect("evaluates");
        assert_eq!(report.failures.len(), 1);
        assert!(report.failures[0].contains("kp-core"), "{report}");
    }

    #[test]
    fn files_of_one_crate_are_summed_not_gated_individually() {
        let mut files = all_green();
        // 0%-covered file, but the crate stays at 500/600 = 83%.
        files.push(("kp-index/src/opt_in.rs", 100, 0));
        files.push(("kp-index/src/hot.rs", 400, 400));
        let report = evaluate(&export(&files)).expect("evaluates");
        assert!(report.failures.is_empty(), "{:?}", report.failures);
    }

    #[test]
    fn ungated_crates_report_but_never_fail() {
        let mut files = all_green();
        files.push(("kp-cli/src/main.rs", 1000, 1)); // 0.1%
        let report = evaluate(&export(&files)).expect("evaluates");
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert!(report.per_crate.contains_key("kp-cli"));
    }

    #[test]
    fn a_gated_crate_missing_from_the_export_fails_loudly() {
        let files: Vec<_> = all_green()
            .into_iter()
            .filter(|(n, _, _)| !n.starts_with("kp-librarian"))
            .collect();
        let report = evaluate(&export(&files)).expect("evaluates");
        assert_eq!(report.failures.len(), 1);
        assert!(report.failures[0].contains("kp-librarian"));
    }

    #[test]
    fn out_of_workspace_files_are_ignored_and_xtask_is_reported() {
        let mut json: serde_json::Value = serde_json::from_str(&export(&all_green())).unwrap();
        let files = json["data"][0]["files"].as_array_mut().unwrap();
        files.push(serde_json::json!({
            "filename": "/home/u/.cargo/registry/src/serde/lib.rs",
            "summary": {"regions": {"count": 10, "covered": 0}}
        }));
        files.push(serde_json::json!({
            "filename": "/ws/xtask/src/litmus.rs",
            "summary": {"regions": {"count": 10, "covered": 9}}
        }));
        let report = evaluate(&json.to_string()).expect("evaluates");
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert_eq!(report.per_crate.len(), 4, "{report}"); // 3 gated + xtask
        assert!(report.per_crate.contains_key("xtask"));
    }

    #[test]
    fn refuses_non_export_json() {
        assert!(evaluate("{\"type\": \"something-else\"}").is_err());
        assert!(evaluate("not json").is_err());
    }

    #[test]
    fn crate_of_maps_paths() {
        assert_eq!(crate_of("/w/crates/kp-core/src/id.rs"), Some("kp-core"));
        assert_eq!(crate_of("/w/xtask/src/main.rs"), Some("xtask"));
        assert_eq!(crate_of("/registry/serde/src/lib.rs"), None);
    }
}
