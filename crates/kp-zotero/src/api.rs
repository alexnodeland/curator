//! Channel 1 — the Zotero Web API v3 metadata client.
//!
//! Delta polling per the API's version protocol:
//! - every request carries `Zotero-API-Version: 3` and the key;
//! - `GET /users/{id}/items?since={v}` with `If-Modified-Since-Version`
//!   answers `304 Not Modified` when the library hasn't moved;
//! - the `Last-Modified-Version` response header is the new cursor;
//! - pagination follows the `Link: <...>; rel="next"` header;
//! - `GET /users/{id}/deleted?since={v}` returns tombstones.
//!
//! The client is blocking (reqwest's blocking API, rustls) — sync is a
//! batch operation, and the rest of the workspace is deliberately
//! async-free. All tests are wiremock-driven against recorded fixtures.

use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder, Response};

use crate::error::ZoteroError;
use crate::item::{Deleted, Fulltext, Item};

/// The API version this client speaks.
pub const API_VERSION: &str = "3";

/// Page size for item listings (the API's maximum).
pub const PAGE_LIMIT: u32 = 100;

/// The result of one delta poll over `/items`.
#[derive(Debug, Clone)]
pub struct ItemsDelta {
    /// Every item object changed since the cursor (all object types:
    /// top-level items, attachments, child notes).
    pub items: Vec<Item>,
    /// The library version to persist as the next cursor.
    pub version: i64,
    /// `true` when the server answered `304 Not Modified` — `items` is
    /// empty and `version` echoes the cursor sent.
    pub not_modified: bool,
}

/// The Zotero Web API v3 client for one user library.
#[derive(Debug, Clone)]
pub struct ZoteroApi {
    http: Client,
    /// e.g. `https://api.zotero.org` (no trailing slash).
    base: String,
    user_id: String,
    api_key: String,
}

impl ZoteroApi {
    /// Build a client. `base` may carry a trailing slash; it is trimmed.
    #[must_use]
    pub fn new(base: &str, user_id: &str, api_key: &str) -> Self {
        Self {
            http: Client::new(),
            base: base.trim_end_matches('/').to_owned(),
            user_id: user_id.to_owned(),
            api_key: api_key.to_owned(),
        }
    }

    /// The underlying HTTP client (shared with the WebDAV shim).
    #[must_use]
    pub fn http(&self) -> &Client {
        &self.http
    }

    fn get(&self, url: &str) -> RequestBuilder {
        self.http
            .get(url)
            .header("Zotero-API-Version", API_VERSION)
            .header("Zotero-API-Key", &self.api_key)
    }

    fn library_url(&self, suffix: &str) -> String {
        format!("{}/users/{}/{suffix}", self.base, self.user_id)
    }

    /// Delta-poll `/items`. `since = None` is the initial full sync;
    /// `Some(v)` sends both `since=v` and `If-Modified-Since-Version: v`
    /// so an unchanged library costs one cheap 304.
    pub fn items_since(&self, since: Option<i64>) -> Result<ItemsDelta, ZoteroError> {
        let mut url = self.library_url(&format!(
            "items?format=json&limit={PAGE_LIMIT}&since={}",
            since.unwrap_or(0)
        ));
        let mut items: Vec<Item> = Vec::new();
        let mut version: Option<i64> = None;
        let mut first = true;
        loop {
            let mut req = self.get(&url);
            if let (true, Some(v)) = (first, since) {
                req = req.header("If-Modified-Since-Version", v.to_string());
            }
            let resp = req.send()?;
            if first && resp.status() == StatusCode::NOT_MODIFIED {
                if let Some(v) = since {
                    return Ok(ItemsDelta {
                        items,
                        version: v,
                        not_modified: true,
                    });
                }
            }
            let resp = ok_or_status(resp, &url)?;
            if version.is_none() {
                version = header_i64(&resp, "Last-Modified-Version");
            }
            let next = next_link(&resp);
            let page: Vec<Item> = resp.json()?;
            items.extend(page);
            match next {
                Some(n) => {
                    url = n;
                    first = false;
                }
                None => break,
            }
        }
        Ok(ItemsDelta {
            items,
            version: version.ok_or(ZoteroError::MissingVersion)?,
            not_modified: false,
        })
    }

    /// Tombstones since a version: the item keys deleted from the library.
    pub fn deleted_since(&self, since: i64) -> Result<Deleted, ZoteroError> {
        let url = self.library_url(&format!("deleted?since={since}"));
        let resp = ok_or_status(self.get(&url).send()?, &url)?;
        Ok(resp.json()?)
    }

    /// Fetch one item by key. `Ok(None)` when the item does not exist.
    pub fn item(&self, key: &str) -> Result<Option<Item>, ZoteroError> {
        let url = self.library_url(&format!("items/{key}?format=json"));
        let resp = self.get(&url).send()?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = ok_or_status(resp, &url)?;
        Ok(Some(resp.json()?))
    }

    /// An item's direct children (attachments + child notes).
    pub fn children(&self, key: &str) -> Result<Vec<Item>, ZoteroError> {
        let url = self.library_url(&format!("items/{key}/children?format=json"));
        let resp = ok_or_status(self.get(&url).send()?, &url)?;
        Ok(resp.json()?)
    }

    /// Channel 2 primary — the official fulltext endpoint for an
    /// attachment. `Ok(None)` when no fulltext has been indexed (404).
    pub fn fulltext(&self, key: &str) -> Result<Option<Fulltext>, ZoteroError> {
        let url = self.library_url(&format!("items/{key}/fulltext"));
        let resp = self.get(&url).send()?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = ok_or_status(resp, &url)?;
        Ok(Some(resp.json()?))
    }
}

/// Surface non-2xx statuses as typed errors with the offending URL.
fn ok_or_status(resp: Response, url: &str) -> Result<Response, ZoteroError> {
    if resp.status().is_success() {
        Ok(resp)
    } else {
        Err(ZoteroError::Status {
            url: url.to_owned(),
            status: resp.status().as_u16(),
        })
    }
}

fn header_i64(resp: &Response, name: &str) -> Option<i64> {
    resp.headers().get(name)?.to_str().ok()?.trim().parse().ok()
}

/// The `rel="next"` target of a `Link` header, if any.
fn next_link(resp: &Response) -> Option<String> {
    let raw = resp.headers().get("Link")?.to_str().ok()?;
    parse_link_next(raw)
}

/// Parse `Link: <url>; rel="alternate", <url>; rel="next", ...`.
#[must_use]
pub fn parse_link_next(header: &str) -> Option<String> {
    for part in header.split(',') {
        let part = part.trim();
        let url = part
            .split(';')
            .next()?
            .trim()
            .strip_prefix('<')?
            .strip_suffix('>')?;
        let is_next = part
            .split(';')
            .skip(1)
            .any(|p| p.trim().eq_ignore_ascii_case("rel=\"next\""));
        if is_next {
            return Some(url.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_header_next_extraction() {
        let header = "<https://example.test/items?start=100>; rel=\"next\", \
                      <https://example.test/items?start=900>; rel=\"last\"";
        assert_eq!(
            parse_link_next(header).as_deref(),
            Some("https://example.test/items?start=100")
        );
        // No next → None.
        assert_eq!(
            parse_link_next("<https://example.test/items>; rel=\"last\""),
            None
        );
        assert_eq!(parse_link_next(""), None);
        // rel comes before another param, case-insensitive.
        assert_eq!(parse_link_next("<u>; REL=\"NEXT\"").as_deref(), Some("u"));
    }

    #[test]
    fn base_trailing_slash_is_trimmed() {
        let api = ZoteroApi::new("https://example.test/", "42", "k");
        assert_eq!(
            api.library_url("items"),
            "https://example.test/users/42/items"
        );
    }
}
