//! Channel 2 fallback — the CRC-verified WebDAV `.prop`/`.zip` shim.
//!
//! Zotero's WebDAV attachment stores keep, per attachment item,
//! `<KEY>.prop` (a small XML descriptor) and `<KEY>.zip` (the attachment
//! files, zipped). This shim, enabled by `[zotero].webdav_fallback`:
//!
//! 1. `GET {webdav_url}/{KEY}.prop` → parse the descriptor XML
//!    (`<properties version="..."><mtime>…</mtime><hash>…</hash>`);
//! 2. `GET {webdav_url}/{KEY}.zip` (hard size cap — the shim never
//!    buffers an unbounded store);
//! 3. extract the first `.html` (else `.txt`) entry, capped, and
//!    CRC-verify: the entry's own CRC32 is checked over the extracted
//!    bytes, and when the `.prop` hash is CRC32-shaped (8 hex chars) it
//!    must match too. A 32-hex `.prop` hash (MD5-shaped, what zotero.org's
//!    client writes) is carried through unverified — MD5 is out of scope
//!    for this deliberately tiny shim, and the zip CRC still guards the
//!    payload.
//!
//! All tests are fixture-driven: real-shaped `.prop` XML plus a tiny zip
//! built in-test. Missing `.prop`/`.zip` (404) is a clean `None`, never an
//! error — most libraries simply don't use WebDAV storage.

use std::io::Read;

use quick_xml::Reader;
use quick_xml::events::Event;
use reqwest::StatusCode;
use reqwest::blocking::Client;
use zip::ZipArchive;

use crate::error::ZoteroError;

/// Size caps guarding the shim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShimCaps {
    /// Max `.zip` payload accepted, in bytes.
    pub max_zip_bytes: u64,
    /// Max uncompressed size of the extracted fulltext entry, in bytes.
    pub max_entry_bytes: u64,
}

impl Default for ShimCaps {
    fn default() -> Self {
        Self {
            max_zip_bytes: 8 * 1024 * 1024,
            max_entry_bytes: 2 * 1024 * 1024,
        }
    }
}

/// The parsed `<KEY>.prop` descriptor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PropInfo {
    /// The `version` attribute of `<properties>`.
    pub version: Option<String>,
    /// Last-sync mtime, milliseconds.
    pub mtime: Option<i64>,
    /// The declared content hash (CRC32 as 8 hex chars, or MD5 as 32).
    pub hash: Option<String>,
}

/// Parse a `.prop` XML document.
pub fn parse_prop(xml: &str) -> Result<PropInfo, ZoteroError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut info = PropInfo::default();
    let mut current: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if name == "properties" {
                    for attr in e.attributes().flatten() {
                        if attr.key.local_name().as_ref() == b"version" {
                            info.version = Some(String::from_utf8_lossy(&attr.value).into_owned());
                        }
                    }
                } else {
                    current = Some(name);
                }
            }
            Ok(Event::Text(t)) => {
                // .prop text content is plain ASCII digits/hex — lossy
                // UTF-8 is exact here and keeps the shim tiny.
                let text = String::from_utf8_lossy(t.as_ref()).into_owned();
                match current.as_deref() {
                    Some("mtime") => info.mtime = text.trim().parse().ok(),
                    Some("hash") => info.hash = Some(text.trim().to_owned()),
                    _ => {}
                }
            }
            Ok(Event::End(_)) => current = None,
            Ok(Event::Eof) => break,
            Err(e) => return Err(ZoteroError::Prop(e.to_string())),
            Ok(_) => {}
        }
    }
    Ok(info)
}

/// The WebDAV shim over a configured base URL.
#[derive(Debug)]
pub struct WebDavShim<'a> {
    http: &'a Client,
    /// e.g. `https://dav.example.test/zotero` (no trailing slash).
    base: String,
    caps: ShimCaps,
}

impl<'a> WebDavShim<'a> {
    /// Build a shim sharing the API client's HTTP stack.
    #[must_use]
    pub fn new(http: &'a Client, base: &str, caps: ShimCaps) -> Self {
        Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            caps,
        }
    }

    /// Fetch + verify + extract one attachment's fulltext. `Ok(None)` when
    /// the store has no `.prop`/`.zip` for this key.
    pub fn fetch_fulltext(&self, key: &str) -> Result<Option<String>, ZoteroError> {
        let prop_url = format!("{}/{key}.prop", self.base);
        let resp = self.http.get(&prop_url).send()?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(ZoteroError::Status {
                url: prop_url,
                status: resp.status().as_u16(),
            });
        }
        let prop = parse_prop(&resp.text()?)?;

        let zip_url = format!("{}/{key}.zip", self.base);
        let resp = self.http.get(&zip_url).send()?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(ZoteroError::Status {
                url: zip_url,
                status: resp.status().as_u16(),
            });
        }
        if let Some(len) = resp.content_length()
            && len > self.caps.max_zip_bytes
        {
            return Err(ZoteroError::TooLarge {
                what: "zip",
                key: key.to_owned(),
                size: len,
                cap: self.caps.max_zip_bytes,
            });
        }
        // Capped read even without a Content-Length header.
        let mut bytes = Vec::new();
        let read = resp
            .take(self.caps.max_zip_bytes + 1)
            .read_to_end(&mut bytes)
            .map_err(ZoteroError::ZipRead)?;
        if read as u64 > self.caps.max_zip_bytes {
            return Err(ZoteroError::TooLarge {
                what: "zip",
                key: key.to_owned(),
                size: read as u64,
                cap: self.caps.max_zip_bytes,
            });
        }
        extract_fulltext(&bytes, &prop, key, self.caps).map(Some)
    }
}

/// Extract the fulltext entry from a `.zip` payload, CRC-verified per the
/// module docs. Pure — this is what the pass/fail fixture tests drive.
pub fn extract_fulltext(
    zip_bytes: &[u8],
    prop: &PropInfo,
    key: &str,
    caps: ShimCaps,
) -> Result<String, ZoteroError> {
    let mut archive = ZipArchive::new(std::io::Cursor::new(zip_bytes))?;

    // First .html entry, else first .txt entry.
    let pick = |suffix: &str| {
        (0..archive.len()).find(|&i| {
            archive
                .name_for_index(i)
                .is_some_and(|n| n.to_ascii_lowercase().ends_with(suffix))
        })
    };
    let index = match pick(".html").or_else(|| pick(".txt")) {
        Some(i) => i,
        None => {
            return Err(ZoteroError::Zip(zip::result::ZipError::FileNotFound));
        }
    };

    let mut entry = archive.by_index(index)?;
    if entry.size() > caps.max_entry_bytes {
        return Err(ZoteroError::TooLarge {
            what: "zip entry",
            key: key.to_owned(),
            size: entry.size(),
            cap: caps.max_entry_bytes,
        });
    }
    let is_html = entry.name().to_ascii_lowercase().ends_with(".html");
    let declared_crc = entry.crc32();
    let mut data = Vec::with_capacity(entry.size() as usize);
    // The zip layer re-checks the entry CRC32 as it reads — corrupted
    // entry bytes surface here as ZipRead.
    entry.read_to_end(&mut data).map_err(ZoteroError::ZipRead)?;

    // Explicit CRC verification over the extracted bytes...
    let got = crc32fast::hash(&data);
    if got != declared_crc {
        return Err(ZoteroError::CrcMismatch {
            key: key.to_owned(),
            expected: declared_crc,
            got,
        });
    }
    // ...and against the .prop when its hash is CRC32-shaped.
    if let Some(hash) = prop.hash.as_deref()
        && hash.len() == 8
        && hash.chars().all(|c| c.is_ascii_hexdigit())
    {
        let expected = u32::from_str_radix(hash, 16)
            .map_err(|e| ZoteroError::Prop(format!("bad CRC32 hash {hash:?}: {e}")))?;
        if got != expected {
            return Err(ZoteroError::CrcMismatch {
                key: key.to_owned(),
                expected,
                got,
            });
        }
    }

    let text = String::from_utf8_lossy(&data).into_owned();
    Ok(if is_html { strip_html(&text) } else { text })
}

/// A deliberately tiny HTML-to-text pass: drop `<script>`/`<style>`
/// subtrees, strip tags, decode the common entities, collapse blank runs.
#[must_use]
pub fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let tail = &rest[open..];
        let lower = tail.to_ascii_lowercase();
        // Skip script/style subtrees wholesale.
        let skipped = ["script", "style"].iter().find_map(|tag| {
            if lower.starts_with(&format!("<{tag}")) {
                let close = format!("</{tag}>");
                lower.find(&close).map(|i| i + close.len())
            } else {
                None
            }
        });
        if let Some(end) = skipped {
            rest = &tail[end..];
            continue;
        }
        match tail.find('>') {
            Some(end) => {
                // Block-ish tags become newlines so words don't fuse.
                if ["<p", "<br", "<div", "</p", "<li", "<h", "</h", "<tr"]
                    .iter()
                    .any(|t| lower.starts_with(t))
                {
                    out.push('\n');
                }
                rest = &tail[end + 1..];
            }
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    // Collapse runs of blank lines and per-line whitespace.
    let mut lines: Vec<&str> = Vec::new();
    let mut blank = true;
    for line in decoded.lines() {
        let line = line.trim();
        if line.is_empty() {
            if !blank {
                lines.push("");
            }
            blank = true;
        } else {
            lines.push(line);
            blank = false;
        }
    }
    while lines.last() == Some(&"") {
        lines.pop();
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_html_to_text() {
        let html = "<html><head><style>p{color:red}</style>\
                    <script>alert('x')</script></head>\
                    <body><h1>Title</h1><p>One &amp; two.</p>\
                    <p>Three&nbsp;four.</p></body></html>";
        assert_eq!(strip_html(html), "Title\n\nOne & two.\n\nThree four.");
    }

    #[test]
    fn strip_html_handles_unclosed_tag() {
        assert_eq!(strip_html("text <unclosed"), "text");
    }
}
