//! The `kp-zotero:managed` comment-marker region.
//!
//! Same philosophy as Curio's managed region: everything between the
//! markers is machine content this producer re-renders on every sync;
//! everything OUTSIDE the markers (and any extra frontmatter keys) belongs
//! to the user and is preserved byte-for-byte. A file whose user zones are
//! empty is *pristine* — only pristine files may ever be deleted on a
//! tombstone; anything the user touched moves to `.kp/trash/` instead.

use kp_core::Note;
use kp_core::note::Frontmatter;

/// Managed-region opening marker (v1).
pub const MANAGED_BEGIN: &str = "<!-- kp-zotero:managed:begin v1 -->";
/// Managed-region closing marker.
pub const MANAGED_END: &str = "<!-- kp-zotero:managed:end -->";

/// The managed/user split of a note body. Reconstruction invariant:
/// `before + MANAGED_BEGIN + managed + MANAGED_END + after == body`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedSplit {
    /// User content above the region (usually empty).
    pub before: String,
    /// The machine-rendered region (between the markers).
    pub managed: String,
    /// User content below the region — the enrichment zone.
    pub after: String,
}

/// Split a body on the v1 markers. `None` when the markers are absent or
/// malformed (missing end, end before begin) — such a file is user-owned.
#[must_use]
pub fn split_managed(body: &str) -> Option<ManagedSplit> {
    let begin = body.find(MANAGED_BEGIN)?;
    let after_begin = begin + MANAGED_BEGIN.len();
    let end_rel = body[after_begin..].find(MANAGED_END)?;
    let end = after_begin + end_rel;
    Some(ManagedSplit {
        before: body[..begin].to_owned(),
        managed: body[after_begin..end].to_owned(),
        after: body[end + MANAGED_END.len()..].to_owned(),
    })
}

/// Compose a body from user zones + fresh managed content.
#[must_use]
pub fn compose_body(before: &str, managed: &str, after: &str) -> String {
    format!("{before}{MANAGED_BEGIN}{managed}{MANAGED_END}{after}")
}

/// Is this file still fully machine-owned? Pristine means: parses as a
/// kp-note, carries no extra frontmatter keys, has intact markers, and
/// both user zones are whitespace-only. Anything else — including a file
/// that no longer parses — counts as user-edited and must never be
/// deleted, only trashed.
#[must_use]
pub fn is_pristine(rel_path: &str, content: &str) -> bool {
    let Ok(note) = Note::parse(rel_path, content) else {
        return false;
    };
    let Frontmatter::Kp(fm) = &note.frontmatter else {
        return false;
    };
    if !fm.extra.is_empty() {
        return false;
    }
    let Some(split) = split_managed(&note.body) else {
        return false;
    };
    split.before.trim().is_empty() && split.after.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine_note(after: &str) -> String {
        format!(
            "---\nkp_id: \"zotero:ABCD2345\"\nkp_schema: kp-note/v1\ntitle: T\n---\n{MANAGED_BEGIN}\n# T\n{MANAGED_END}{after}"
        )
    }

    #[test]
    fn split_reconstructs_the_body() {
        let body = format!("intro\n{MANAGED_BEGIN}\nmachine\n{MANAGED_END}\nuser notes\n");
        let split = split_managed(&body).expect("markers present");
        assert_eq!(split.before, "intro\n");
        assert_eq!(split.managed, "\nmachine\n");
        assert_eq!(split.after, "\nuser notes\n");
        assert_eq!(
            compose_body(&split.before, &split.managed, &split.after),
            body
        );
    }

    #[test]
    fn malformed_markers_mean_no_split() {
        assert_eq!(split_managed("no markers"), None);
        assert_eq!(split_managed(&format!("{MANAGED_BEGIN}\nopen")), None);
        assert_eq!(
            split_managed(&format!("{MANAGED_END}\n{MANAGED_BEGIN}")),
            None
        );
    }

    #[test]
    fn pristine_detection() {
        assert!(is_pristine("z/a.md", &machine_note("\n")));
        // User content below the region.
        assert!(!is_pristine("z/a.md", &machine_note("\nmy thoughts\n")));
        // User content above the region.
        let above = machine_note("\n").replace(MANAGED_BEGIN, &format!("mine\n{MANAGED_BEGIN}"));
        assert!(!is_pristine("z/a.md", &above));
        // Extra frontmatter keys are a user edit.
        let extra = machine_note("\n").replace("title: T\n", "title: T\naliases: [x]\n");
        assert!(!is_pristine("z/a.md", &extra));
        // Markers removed entirely.
        let stripped = machine_note("\n")
            .replace(MANAGED_BEGIN, "")
            .replace(MANAGED_END, "");
        assert!(!is_pristine("z/a.md", &stripped));
        // Unparseable file: user-mangled, never pristine.
        assert!(!is_pristine("z/a.md", "---\nkp_id: \"zotero:X\"\nno close"));
        // Foreign frontmatter (kp block stripped).
        assert!(!is_pristine(
            "z/a.md",
            &format!("---\nother: tool\n---\n{MANAGED_BEGIN}x{MANAGED_END}\n")
        ));
    }
}
