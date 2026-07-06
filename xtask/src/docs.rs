//! The docs-site generator.
//!
//! `cargo run -p xtask -- docs` renders `docs/site/` (markdown pages +
//! `nav.json` + `assets/`) into a self-contained static site at
//! `target/site/` — the GitHub Pages payload. Design goals, in order:
//!
//! 1. **Deterministic.** Same sources → byte-identical site. No
//!    timestamps, no environment probes, no unordered iteration: page
//!    order comes from `nav.json`, asset order from a sorted directory
//!    read. CI builds twice and diffs.
//! 2. **Zero external tooling.** The generator is this file plus
//!    `pulldown-cmark` (pinned via `Cargo.lock`); a clean runner with
//!    nothing but Rust builds the site — the same hermetic promise the
//!    test suite makes.
//! 3. **Self-contained output.** Mermaid is vendored
//!    (`docs/site/assets/mermaid.min.js`, pinned — see
//!    `docs/site/assets/README.md`) and loaded only by pages that
//!    contain a mermaid fence. No CDN, no fonts, no network at view
//!    time.
//!
//! The generator is also a gate: it fails (nonzero exit) on a nav entry
//! without a file, a page without an `# H1`, a relative `*.md` link
//! that does not resolve to a page in the nav, or a `#fragment` that
//! does not match a heading anchor on the target page — a broken docs
//! cross-reference cannot reach `main`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use pulldown_cmark::{
    CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd, html,
};

/// Site sources, workspace-relative.
const SITE_DIR: &str = "docs/site";
/// Build output, workspace-relative (gitignored via `target/`, and
/// skipped by the litmus for the same reason).
const OUT_DIR: &str = "target/site";

/// Entry point for `cargo run -p xtask -- docs [root]`.
pub fn run(root: Option<&str>) -> ExitCode {
    let root = root.map(PathBuf::from).unwrap_or_else(workspace_root);
    match build(&root) {
        Ok(pages) => {
            println!(
                "docs: {pages} page(s) rendered into {}",
                root.join(OUT_DIR).display()
            );
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("docs: {message}");
            ExitCode::FAILURE
        }
    }
}

/// The workspace root: this file lives at `<root>/xtask/src/docs.rs`.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits one level under the workspace root")
        .to_path_buf()
}

/// One nav entry: a markdown page and its sidebar label.
struct NavPage {
    /// Site-relative markdown path, `/`-separated (e.g. `reference/cli.md`).
    file: String,
    /// Sidebar label.
    label: String,
}

/// One sidebar section.
struct NavSection {
    title: String,
    pages: Vec<NavPage>,
}

/// Build the whole site. Returns the number of pages rendered.
fn build(root: &Path) -> Result<usize, String> {
    let site = root.join(SITE_DIR);
    let out = root.join(OUT_DIR);
    let sections = load_nav(&site.join("nav.json"))?;

    // The page set — every internal `*.md` link must land in it.
    let page_set: BTreeSet<String> = sections
        .iter()
        .flat_map(|s| s.pages.iter().map(|p| p.file.clone()))
        .collect();

    // Pass 1: convert every page, collecting heading anchors and the
    // fragment links to check against them.
    let mut converted: BTreeMap<String, Converted> = BTreeMap::new();
    for section in &sections {
        for page in &section.pages {
            let src = site.join("src").join(&page.file);
            let markdown = fs::read_to_string(&src)
                .map_err(|e| format!("nav names {} but: {e}", src.display()))?;
            let one = convert_markdown(&markdown, &page.file, &page_set)?;
            if one.title.is_none() {
                return Err(format!(
                    "{}: no `# H1` heading — every page needs one",
                    page.file
                ));
            }
            converted.insert(page.file.clone(), one);
        }
    }

    // Pass 2: every `#fragment` must be a heading anchor on its target.
    for (file, page) in &converted {
        for link in &page.fragment_links {
            let target = converted
                .get(&link.target)
                .expect("existence checked during conversion");
            if !target.slugs.contains(&link.fragment) {
                return Err(format!(
                    "{file}: link {:?} — no heading anchor #{} on {}",
                    link.raw, link.fragment, link.target
                ));
            }
        }
    }

    // Pass 3: write the site.
    if out.exists() {
        fs::remove_dir_all(&out).map_err(|e| format!("cannot clear {}: {e}", out.display()))?;
    }
    fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    copy_assets(&site.join("assets"), &out.join("assets"))?;
    let mut rendered = 0;
    for section in &sections {
        for page in &section.pages {
            let one = &converted[&page.file];
            let title = one.title.as_deref().expect("checked in pass 1");
            let html_page = page_shell(title, one, &sections, &page.file);
            let dest = out.join(html_path(&page.file));
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::write(&dest, html_page).map_err(|e| e.to_string())?;
            rendered += 1;
        }
    }
    Ok(rendered)
}

/// Parse `nav.json` — the single source of page order and sidebar labels.
fn load_nav(path: &Path) -> Result<Vec<NavSection>, String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let sections = value["sections"]
        .as_array()
        .ok_or("nav.json: top-level `sections` array missing")?;
    let mut out = Vec::new();
    for section in sections {
        let title = section["title"]
            .as_str()
            .ok_or("nav.json: section without a string `title`")?
            .to_owned();
        let mut pages = Vec::new();
        for page in section["pages"]
            .as_array()
            .ok_or_else(|| format!("nav.json: section {title:?} without a `pages` array"))?
        {
            pages.push(NavPage {
                file: page["file"]
                    .as_str()
                    .ok_or("nav.json: page without a string `file`")?
                    .to_owned(),
                label: page["label"]
                    .as_str()
                    .ok_or("nav.json: page without a string `label`")?
                    .to_owned(),
            });
        }
        out.push(NavSection { title, pages });
    }
    Ok(out)
}

/// Copy `assets/` verbatim, in sorted order (determinism).
fn copy_assets(src: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let mut names: Vec<_> = fs::read_dir(src)
        .map_err(|e| format!("cannot read {}: {e}", src.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name())
        .collect();
    names.sort();
    for name in names {
        // The vendoring provenance note is a source artifact, not a
        // page asset.
        if name.to_string_lossy() == "README.md" {
            continue;
        }
        fs::copy(src.join(&name), dest.join(&name)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// A `#fragment` reference awaiting validation against its target page.
#[derive(Debug)]
struct FragmentLink {
    /// The link as written (for error messages).
    raw: String,
    /// Site-relative target page (may be the linking page itself).
    target: String,
    /// The fragment, without `#`.
    fragment: String,
}

/// A converted page body plus what the shell and the link checker need.
#[derive(Debug)]
struct Converted {
    body: String,
    title: Option<String>,
    has_mermaid: bool,
    /// Heading anchors generated on this page.
    slugs: BTreeSet<String>,
    fragment_links: Vec<FragmentLink>,
}

/// Markdown → HTML with three transforms on the event stream:
///
/// - ```` ```mermaid ```` fences become `<pre class="mermaid">` blocks
///   (rendered client-side by the vendored mermaid bundle);
/// - relative `*.md` links are rewritten to `*.html` and checked
///   against the page set (fragments are recorded for pass 2);
/// - headings get GitHub-style `id` anchors derived from their text.
fn convert_markdown(
    markdown: &str,
    page: &str,
    page_set: &BTreeSet<String>,
) -> Result<Converted, String> {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_FOOTNOTES;
    let parser = Parser::new_ext(markdown, options);

    let mut events: Vec<Event> = Vec::new();
    let mut has_mermaid = false;
    let mut mermaid_buf: Option<String> = None;
    let mut fragment_links = Vec::new();

    for event in parser {
        // Inside a mermaid fence: swallow text into the buffer.
        if let Some(buf) = mermaid_buf.as_mut() {
            match event {
                Event::Text(text) => {
                    buf.push_str(&text);
                    continue;
                }
                Event::End(TagEnd::CodeBlock) => {
                    let block = mermaid_buf.take().expect("buffer present");
                    has_mermaid = true;
                    events.push(Event::Html(CowStr::from(format!(
                        "<pre class=\"mermaid\">{}</pre>",
                        escape_html(&block)
                    ))));
                    continue;
                }
                other => {
                    return Err(format!(
                        "{page}: unexpected event inside mermaid fence: {other:?}"
                    ));
                }
            }
        }
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(ref lang)))
                if lang.as_ref() == "mermaid" =>
            {
                mermaid_buf = Some(String::new());
            }
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => {
                let rewritten = rewrite_link(&dest_url, page, page_set, &mut fragment_links)?;
                events.push(Event::Start(Tag::Link {
                    link_type,
                    dest_url: CowStr::from(rewritten),
                    title,
                    id,
                }));
            }
            other => events.push(other),
        }
    }

    // Heading pass: derive anchors from heading text, dedupe, record;
    // the first H1's text is the page title.
    let mut slugs = BTreeSet::new();
    let mut title: Option<String> = None;
    let mut i = 0;
    while i < events.len() {
        if let Event::Start(Tag::Heading { level, id, .. }) = &events[i] {
            let level = *level;
            let explicit = id.clone();
            let mut text = String::new();
            let mut j = i + 1;
            while j < events.len() {
                match &events[j] {
                    Event::Text(t) => text.push_str(t),
                    Event::Code(t) => text.push_str(t),
                    Event::End(TagEnd::Heading(_)) => break,
                    _ => {}
                }
                j += 1;
            }
            let slug = match explicit {
                Some(s) => s.to_string(),
                None => {
                    let base = slugify(&text);
                    let mut candidate = base.clone();
                    let mut n = 1;
                    while slugs.contains(&candidate) {
                        candidate = format!("{base}-{n}");
                        n += 1;
                    }
                    candidate
                }
            };
            slugs.insert(slug.clone());
            events[i] = Event::Start(Tag::Heading {
                level,
                id: Some(CowStr::from(slug)),
                classes: Vec::new(),
                attrs: Vec::new(),
            });
            if title.is_none() && level == HeadingLevel::H1 {
                title = Some(text);
            }
        }
        i += 1;
    }

    let mut body = String::new();
    html::push_html(&mut body, events.into_iter());
    Ok(Converted {
        body,
        title,
        has_mermaid,
        slugs,
        fragment_links,
    })
}

/// Rewrite a relative `*.md` link to its `*.html` output path, checking
/// it resolves to a page in the nav and recording any fragment for
/// pass-2 anchor validation. External and non-markdown links pass
/// through untouched; same-page `#fragment` links are recorded too.
fn rewrite_link(
    dest: &str,
    page: &str,
    page_set: &BTreeSet<String>,
    fragment_links: &mut Vec<FragmentLink>,
) -> Result<String, String> {
    if dest.contains("://") || dest.starts_with("mailto:") {
        return Ok(dest.to_owned());
    }
    if let Some(fragment) = dest.strip_prefix('#') {
        fragment_links.push(FragmentLink {
            raw: dest.to_owned(),
            target: page.to_owned(),
            fragment: fragment.to_owned(),
        });
        return Ok(dest.to_owned());
    }
    let (path, fragment) = match dest.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (dest, None),
    };
    if !path.ends_with(".md") {
        return Ok(dest.to_owned());
    }
    // Resolve relative to the linking page's directory, then normalize.
    let base = match page.rsplit_once('/') {
        Some((dir, _)) => dir,
        None => "",
    };
    let resolved = normalize_rel(base, path)
        .ok_or_else(|| format!("{page}: link {dest:?} escapes the site root"))?;
    if !page_set.contains(&resolved) {
        return Err(format!(
            "{page}: link {dest:?} resolves to {resolved:?}, which is not a page in nav.json"
        ));
    }
    let mut out = format!("{}.html", path.strip_suffix(".md").expect("checked above"));
    if let Some(fragment) = fragment {
        fragment_links.push(FragmentLink {
            raw: dest.to_owned(),
            target: resolved,
            fragment: fragment.to_owned(),
        });
        let _ = write!(out, "#{fragment}");
    }
    Ok(out)
}

/// GitHub-style heading slug: lowercase, alphanumeric runs joined by
/// single hyphens.
fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut pending_hyphen = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if pending_hyphen && !out.is_empty() {
                out.push('-');
            }
            pending_hyphen = false;
            out.extend(c.to_lowercase());
        } else {
            pending_hyphen = true;
        }
    }
    out
}

/// Join `base` (a site-relative directory, possibly empty) with a
/// relative `path`, resolving `.`/`..`. `None` if it escapes the root.
fn normalize_rel(base: &str, path: &str) -> Option<String> {
    let mut parts: Vec<&str> = if base.is_empty() {
        Vec::new()
    } else {
        base.split('/').collect()
    };
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

/// `index.md` → `index.html`, `reference/cli.md` → `reference/cli.html`.
fn html_path(page: &str) -> String {
    format!("{}.html", page.strip_suffix(".md").unwrap_or(page))
}

/// `../` prefix that climbs from `page`'s directory back to the site root.
fn rel_prefix(page: &str) -> String {
    "../".repeat(page.matches('/').count())
}

/// Minimal HTML escaping for text embedded in markup.
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Wrap a converted body in the full page shell: head, sidebar, footer,
/// and (only when needed) the vendored mermaid bundle.
fn page_shell(title: &str, converted: &Converted, sections: &[NavSection], page: &str) -> String {
    let prefix = rel_prefix(page);
    // "Quickstart — Curator", but never "Curator — Curator" on the landing page.
    let tab_title = if title == "Curator" {
        title.to_owned()
    } else {
        format!("{title} — Curator")
    };
    let mut nav = String::new();
    for section in sections {
        let _ = write!(nav, "<h2>{}</h2>\n<ul>\n", escape_html(&section.title));
        for entry in &section.pages {
            let class = if entry.file == page {
                " class=\"current\""
            } else {
                ""
            };
            let _ = writeln!(
                nav,
                "<li><a{class} href=\"{prefix}{href}\">{label}</a></li>",
                href = html_path(&entry.file),
                label = escape_html(&entry.label)
            );
        }
        nav.push_str("</ul>\n");
    }
    let mermaid = if converted.has_mermaid {
        format!(
            "<script src=\"{prefix}assets/mermaid.min.js\"></script>\n\
             <script src=\"{prefix}assets/mermaid-init.js\"></script>\n"
        )
    } else {
        String::new()
    };
    format!(
        "<!doctype html>\n\
         <html lang=\"en\">\n\
         <head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{title}</title>\n\
         <link rel=\"icon\" href=\"data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><text y='13' font-size='13'>&#128218;</text></svg>\">\n\
         <link rel=\"stylesheet\" href=\"{prefix}assets/style.css\">\n\
         </head>\n\
         <body>\n\
         <input type=\"checkbox\" id=\"nav-toggle\" hidden>\n\
         <label for=\"nav-toggle\" class=\"nav-button\" aria-label=\"Toggle navigation\">menu</label>\n\
         <nav class=\"sidebar\">\n\
         <div class=\"brand\"><a href=\"{prefix}index.html\">Curator</a></div>\n\
         {nav}\
         </nav>\n\
         <main>\n\
         {body}\
         <footer>Generated from <code>docs/site/</code> by <code>cargo run -p xtask -- docs</code> — \
         deterministic by construction (same sources, same bytes).</footer>\n\
         </main>\n\
         {mermaid}\
         </body>\n\
         </html>\n",
        title = escape_html(&tab_title),
        body = converted.body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn mermaid_fence_becomes_pre_block() {
        let md = "# T\n\n```mermaid\nflowchart LR\n  a --> b\n```\n";
        let out = convert_markdown(md, "index.md", &pages(&["index.md"])).unwrap();
        assert!(out.has_mermaid);
        assert!(
            out.body
                .contains("<pre class=\"mermaid\">flowchart LR\n  a --&gt; b\n</pre>")
        );
        // No stray <code> wrapper — mermaid reads the pre's textContent.
        assert!(!out.body.contains("language-mermaid"));
    }

    #[test]
    fn non_mermaid_fences_stay_code_blocks() {
        let md = "# T\n\n```sh\necho hi\n```\n";
        let out = convert_markdown(md, "index.md", &pages(&["index.md"])).unwrap();
        assert!(!out.has_mermaid);
        assert!(out.body.contains("<code class=\"language-sh\">"));
    }

    #[test]
    fn md_links_rewrite_to_html_and_record_fragments() {
        let md = "# T\n\n[c](../concepts.md#epochs)\n";
        let set = pages(&["concepts.md", "reference/cli.md"]);
        let out = convert_markdown(md, "reference/cli.md", &set).unwrap();
        assert!(out.body.contains("href=\"../concepts.html#epochs\""));
        assert_eq!(out.fragment_links.len(), 1);
        assert_eq!(out.fragment_links[0].target, "concepts.md");
        assert_eq!(out.fragment_links[0].fragment, "epochs");
    }

    #[test]
    fn dangling_md_link_fails_the_build() {
        let md = "# T\n\n[gone](missing.md)\n";
        let err = convert_markdown(md, "index.md", &pages(&["index.md"])).unwrap_err();
        assert!(err.contains("missing.md"), "{err}");
    }

    #[test]
    fn external_links_pass_through_and_same_page_fragments_record() {
        let md = "# T\n\n[x](https://example.com/a.md) [y](#t)\n";
        let out = convert_markdown(md, "index.md", &pages(&["index.md"])).unwrap();
        assert!(out.body.contains("href=\"https://example.com/a.md\""));
        assert!(out.body.contains("href=\"#t\""));
        assert_eq!(out.fragment_links.len(), 1);
        assert_eq!(out.fragment_links[0].target, "index.md");
    }

    #[test]
    fn title_comes_from_first_h1_and_headings_get_anchors() {
        let md = "# The `curator` CLI\n\n## Epochs, not migrations\n";
        let out = convert_markdown(md, "index.md", &pages(&["index.md"])).unwrap();
        assert_eq!(out.title.as_deref(), Some("The curator CLI"));
        assert!(out.slugs.contains("the-curator-cli"));
        assert!(out.slugs.contains("epochs-not-migrations"));
        assert!(out.body.contains("<h2 id=\"epochs-not-migrations\">"));
    }

    #[test]
    fn duplicate_headings_dedupe_their_anchors() {
        let md = "# T\n\n## Setup\n\n## Setup\n";
        let out = convert_markdown(md, "index.md", &pages(&["index.md"])).unwrap();
        assert!(out.slugs.contains("setup"));
        assert!(out.slugs.contains("setup-1"));
    }

    #[test]
    fn normalize_rel_resolves_dotdot_and_rejects_escape() {
        assert_eq!(
            normalize_rel("reference", "../concepts.md").as_deref(),
            Some("concepts.md")
        );
        assert_eq!(
            normalize_rel("", "integrations/curio.md").as_deref(),
            Some("integrations/curio.md")
        );
        assert_eq!(normalize_rel("", "../escape.md"), None);
    }

    #[test]
    fn rel_prefix_matches_depth() {
        assert_eq!(rel_prefix("index.md"), "");
        assert_eq!(rel_prefix("reference/cli.md"), "../");
    }

    #[test]
    fn slugify_is_github_style() {
        assert_eq!(slugify("Epochs, not migrations"), "epochs-not-migrations");
        assert_eq!(slugify("Serving MCP over HTTP"), "serving-mcp-over-http");
        assert_eq!(slugify("  spaced   out  "), "spaced-out");
    }
}
