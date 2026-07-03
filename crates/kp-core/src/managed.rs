//! Curio managed-region markers — the cross-contract constants.
//!
//! Curio owns the region between these HTML-comment markers inside notes
//! it exports (see `contracts/kp-note/v1.md`): KP enrichment is companion
//! content BELOW the region plus additional frontmatter keys outside
//! Curio's machine-key set — never inside. The constants live here, at
//! the bottom of the workspace, because both the ingest adapter (reading
//! the split) and the proposals validator (refusing edits inside it)
//! need them.

/// Managed-region opening marker (v1).
pub const MANAGED_BEGIN: &str = "<!-- curio:managed:begin v1 -->";
/// Managed-region closing marker.
pub const MANAGED_END: &str = "<!-- curio:managed:end -->";
/// The `schema` frontmatter value of a Curio-exported note.
pub const CURIO_FRONTMATTER_SCHEMA: &str = "curio.frontmatter.v1";

/// The byte-exact managed block of a note body — `MANAGED_BEGIN` through
/// `MANAGED_END` inclusive — or `None` when the markers are absent or
/// malformed (end before begin, either missing).
#[must_use]
pub fn managed_block(body: &str) -> Option<&str> {
    let begin = body.find(MANAGED_BEGIN)?;
    let end_rel = body[begin..].find(MANAGED_END)?;
    Some(&body[begin..begin + end_rel + MANAGED_END.len()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_block_inclusive_of_markers() {
        let body = format!("above\n{MANAGED_BEGIN}\ninside\n{MANAGED_END}\nbelow\n");
        let block = managed_block(&body).expect("markers present");
        assert!(block.starts_with(MANAGED_BEGIN));
        assert!(block.ends_with(MANAGED_END));
        assert!(block.contains("inside"));
        assert!(!block.contains("above"));
        assert!(!block.contains("below"));
    }

    #[test]
    fn absent_or_malformed_markers_yield_none() {
        assert_eq!(managed_block("no markers here"), None);
        assert_eq!(managed_block(MANAGED_BEGIN), None); // unterminated
        assert_eq!(managed_block(MANAGED_END), None); // never opened
        let reversed = format!("{MANAGED_END}\n{MANAGED_BEGIN}");
        assert_eq!(managed_block(&reversed), None); // end before begin
    }
}
