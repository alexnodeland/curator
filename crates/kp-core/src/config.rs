//! `kp-config/v1` — `kp.toml` (contract: `contracts/kp-config/v1.md`).
//!
//! Rules: the config is versioned via the `schema` key; unknown keys warn,
//! never fail; secrets are only ever env/keychain indirection — never values
//! in the file.

use serde::{Deserialize, Serialize};

/// The `schema` value this crate implements.
pub const KP_CONFIG_SCHEMA: &str = "kp-config/v1";

/// Top-level `kp.toml` model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KpConfig {
    /// Config contract version, e.g. `kp-config/v1`.
    pub schema: String,
    #[serde(default)]
    pub vault: VaultConfig,
    #[serde(default)]
    pub index: IndexConfig,
    #[serde(default)]
    pub curio: CurioConfig,
    #[serde(default)]
    pub zotero: ZoteroConfig,
    #[serde(default)]
    pub librarian: LibrarianConfig,
    #[serde(default)]
    pub mcp: McpConfig,
}

impl KpConfig {
    /// Parse a `kp.toml` document.
    ///
    /// Unknown keys are ignored (the real loader will surface them as
    /// warnings — per contract they must never be an error).
    pub fn from_toml_str(raw: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(raw)
    }
}

/// `[vault]` — the markdown corpus root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VaultConfig {
    pub path: String,
    /// Relative to the vault.
    pub proposals_dir: String,
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            path: "~/vault".to_owned(),
            proposals_dir: ".kp/proposals".to_owned(),
        }
    }
}

/// `[index]` — the ONE embedded SQLite db: vec + FTS5 + edges.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IndexConfig {
    pub path: String,
    /// `builtin` = in-process pinned CPU ONNX; `hash` = deterministic test embedder.
    pub embedder: String,
    pub chunk_tokens: u32,
    pub chunk_overlap: u32,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            path: "~/.local/share/kp/index.db".to_owned(),
            embedder: "builtin".to_owned(),
            chunk_tokens: 512,
            chunk_overlap: 64,
        }
    }
}

/// `[curio]` — the Curio producer seam.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CurioConfig {
    pub enabled: bool,
    /// Tail target for `curio.events.v1` JSONL (rotation-aware cursors).
    pub events_dir: String,
    /// Vault-relative dirs Curio exports into.
    pub notes_dirs: Vec<String>,
}

impl Default for CurioConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            events_dir: "~/.local/share/curio/events".to_owned(),
            notes_dirs: vec!["curio".to_owned()],
        }
    }
}

/// `[zotero]` — the Zotero producer seam.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZoteroConfig {
    pub enabled: bool,
    pub api_base: String,
    pub user_id: String,
    /// Env var NAME holding the API key — never the key itself.
    pub api_key_env: String,
    pub webdav_fallback: bool,
    pub webdav_url: String,
}

impl Default for ZoteroConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_base: "https://api.zotero.org".to_owned(),
            user_id: String::new(),
            api_key_env: "KP_ZOTERO_KEY".to_owned(),
            webdav_fallback: false,
            webdav_url: String::new(),
        }
    }
}

/// `[librarian]` — deterministic digest tuning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibrarianConfig {
    /// Interest anchor note (vault-relative).
    pub now_path: String,
    /// Vault-relative output dir (`kp:` namespace notes).
    pub digest_dir: String,
    /// Recency decay half-life, in days.
    pub half_life_days: u32,
    pub top_k: u32,
}

impl Default for LibrarianConfig {
    fn default() -> Self {
        Self {
            now_path: "now.md".to_owned(),
            digest_dir: "digests".to_owned(),
            half_life_days: 14,
            top_k: 12,
        }
    }
}

/// `[mcp]` — the one MCP entrypoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpConfig {
    /// `stdio` (default) or `http`.
    pub transport: String,
    pub http_bind: String,
    /// Env var NAME holding the bearer token; required when transport = http.
    pub bearer_token_env: String,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            transport: "stdio".to_owned(),
            http_bind: "127.0.0.1:8377".to_owned(),
            bearer_token_env: "KP_MCP_TOKEN".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_config_gets_contract_defaults() {
        let cfg = KpConfig::from_toml_str("schema = \"kp-config/v1\"\n").expect("should parse");
        assert_eq!(cfg.schema, KP_CONFIG_SCHEMA);
        assert_eq!(cfg.vault.proposals_dir, ".kp/proposals");
        assert_eq!(cfg.index.embedder, "builtin");
        assert_eq!(cfg.index.chunk_tokens, 512);
        assert!(!cfg.curio.enabled);
        assert_eq!(cfg.librarian.half_life_days, 14);
        assert_eq!(cfg.mcp.transport, "stdio");
    }

    #[test]
    fn unknown_keys_never_fail() {
        let raw = "schema = \"kp-config/v1\"\nfuture_key = true\n\n[vault]\npath = \"/tmp/v\"\nfrom_v2 = 3\n";
        let cfg = KpConfig::from_toml_str(raw).expect("unknown keys must not fail");
        assert_eq!(cfg.vault.path, "/tmp/v");
    }
}
