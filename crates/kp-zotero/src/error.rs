//! kp-zotero error types.

use std::path::PathBuf;

/// Errors from the Zotero producer (both channels + sync orchestration).
#[derive(Debug, thiserror::Error)]
pub enum ZoteroError {
    /// The HTTP transport failed (connect, timeout, body decode...).
    #[error("Zotero request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// The server answered with an unexpected status code.
    #[error("Zotero endpoint {url} returned HTTP {status}")]
    Status { url: String, status: u16 },
    /// A 200 items response without the `Last-Modified-Version` header —
    /// the delta cursor cannot advance safely without it.
    #[error("Zotero items response is missing the Last-Modified-Version header")]
    MissingVersion,
    /// The WebDAV `.prop` XML did not parse.
    #[error("malformed WebDAV .prop XML: {0}")]
    Prop(String),
    /// The WebDAV `.zip` payload is not a readable archive.
    #[error("WebDAV zip archive: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// Reading a zip entry failed — including the zip layer's own CRC32
    /// check firing on corrupted entry data.
    #[error("WebDAV zip entry read failed (corrupt data or CRC mismatch): {0}")]
    ZipRead(#[source] std::io::Error),
    /// A WebDAV payload blew through a size cap. The shim never buffers
    /// unbounded attachment stores.
    #[error("WebDAV {what} for {key} is {size} bytes — over the {cap}-byte cap")]
    TooLarge {
        what: &'static str,
        key: String,
        size: u64,
        cap: u64,
    },
    /// The `.prop` file declares a CRC32 that does not match the extracted
    /// content.
    #[error("WebDAV CRC mismatch for {key}: .prop declares {expected:08x}, content is {got:08x}")]
    CrcMismatch {
        key: String,
        expected: u32,
        got: u32,
    },
    /// Vault trouble while upserting or trashing notes.
    #[error(transparent)]
    Vault(#[from] kp_core::VaultError),
    /// Index trouble while reading or persisting the version cursor.
    #[error(transparent)]
    Index(#[from] kp_index::IndexError),
    /// Plain filesystem trouble (tombstone removal).
    #[error("I/O on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
