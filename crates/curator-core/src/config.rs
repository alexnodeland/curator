//! `kp-config/v1` — the config file (contract: `contracts/kp-config/v1.md`).
//!
//! The preferred file name is `curator.toml`; the legacy `kp.toml` name
//! remains accepted (deprecated, never removed within v1). Binding rules
//! implemented here:
//! 1. the config is versioned via the top-level `schema` key;
//! 2. unknown keys warn (via `tracing`), never fail — a config written for
//!    a newer minor version must load on an older binary;
//! 3. secrets only via env/keychain indirection (`*_env` keys name the
//!    variable) — never secret values in the file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The `schema` value this crate implements.
pub const KP_CONFIG_SCHEMA: &str = "kp-config/v1";

/// The preferred config file name.
pub const CONFIG_FILE: &str = "curator.toml";

/// The legacy config file name — still accepted, deprecated (kp-config/v1
/// never removes it).
pub const CONFIG_FILE_LEGACY: &str = "kp.toml";

/// Where [`discover_config`] found the config path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSource {
    /// Named by an environment variable (`CURATOR_CONFIG` preferred,
    /// `KP_CONFIG` honored) — the variable name is carried.
    Env(String),
    /// `./curator.toml` — the preferred file name.
    File,
    /// `./kp.toml` — the legacy file name; callers should surface a
    /// deprecation note.
    LegacyFile,
    /// Nothing found: the preferred name is returned so error paths
    /// report `curator.toml`.
    Default,
}

/// Discover the config path when no explicit `--config` was given:
/// `$CURATOR_CONFIG` (preferred), then `$KP_CONFIG` (legacy, honored),
/// then `./curator.toml`, then `./kp.toml` (deprecated), and finally the
/// preferred default name. Pure — env and filesystem arrive as closures.
pub fn discover_config(
    env: impl Fn(&str) -> Option<String>,
    exists: impl Fn(&str) -> bool,
) -> (PathBuf, ConfigSource) {
    for var in ["CURATOR_CONFIG", "KP_CONFIG"] {
        if let Some(path) = env(var).filter(|v| !v.is_empty()) {
            return (PathBuf::from(path), ConfigSource::Env(var.to_owned()));
        }
    }
    if exists(CONFIG_FILE) {
        return (PathBuf::from(CONFIG_FILE), ConfigSource::File);
    }
    if exists(CONFIG_FILE_LEGACY) {
        return (PathBuf::from(CONFIG_FILE_LEGACY), ConfigSource::LegacyFile);
    }
    (PathBuf::from(CONFIG_FILE), ConfigSource::Default)
}

/// Errors from loading the config file.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read.
    #[error("cannot read config {path}: {source}")]
    Io {
        /// The path that could not be read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The file is not valid TOML or is missing required keys.
    #[error("invalid config: {0}")]
    Parse(#[from] toml::de::Error),
    /// The `schema` key names a contract this binary does not implement.
    /// Unknown *keys* are tolerated; an unknown *schema* is a different
    /// major and must not be silently reinterpreted.
    #[error("unsupported config schema {found:?} (this binary implements {KP_CONFIG_SCHEMA:?})")]
    UnsupportedSchema {
        /// The `schema` value found in the file.
        found: String,
    },
}

/// Top-level config model (`curator.toml`; legacy name `kp.toml`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KpConfig {
    /// Config contract version, e.g. `kp-config/v1`.
    pub schema: String,
    /// `[vault]` — the markdown corpus root.
    #[serde(default)]
    pub vault: VaultConfig,
    /// `[index]` — the embedded index database.
    #[serde(default)]
    pub index: IndexConfig,
    /// `[curio]` — the Curio producer seam.
    #[serde(default)]
    pub curio: CurioConfig,
    /// `[zotero]` — the Zotero producer seam.
    #[serde(default)]
    pub zotero: ZoteroConfig,
    /// `[librarian]` — deterministic digest tuning.
    #[serde(default)]
    pub librarian: LibrarianConfig,
    /// `[mcp]` — the one MCP entrypoint.
    #[serde(default)]
    pub mcp: McpConfig,
}

impl KpConfig {
    /// Load a config file (`curator.toml`, or the legacy `kp.toml`).
    /// Unknown keys are logged as warnings and otherwise ignored; a wrong
    /// `schema` value is an error.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_owned(),
            source,
        })?;
        Self::from_toml_str(&raw)
    }

    /// Parse a config document. Unknown keys warn via `tracing` — per
    /// contract they must never be an error.
    pub fn from_toml_str(raw: &str) -> Result<Self, ConfigError> {
        for key in unknown_keys(raw)? {
            tracing::warn!(key, "unknown config key ignored (newer config minor?)");
        }
        let cfg: Self = toml::from_str(raw)?;
        if cfg.schema != KP_CONFIG_SCHEMA {
            return Err(ConfigError::UnsupportedSchema { found: cfg.schema });
        }
        Ok(cfg)
    }

    /// The vault root, tilde-expanded.
    #[must_use]
    pub fn vault_path(&self) -> PathBuf {
        expand_tilde(&self.vault.path)
    }

    /// The index database path, tilde-expanded.
    #[must_use]
    pub fn index_path(&self) -> PathBuf {
        expand_tilde(&self.index.path)
    }

    /// The Curio events directory, tilde-expanded.
    #[must_use]
    pub fn curio_events_dir(&self) -> PathBuf {
        expand_tilde(&self.curio.events_dir)
    }
}

/// Dotted paths of keys present in `raw` that this config model does not
/// know. Pure — the loader turns each into a `tracing` warning.
pub fn unknown_keys(raw: &str) -> Result<Vec<String>, ConfigError> {
    let doc: toml::Table = toml::from_str(raw)?;
    // The known key sets mirror the contract's normative example exactly.
    const TOP: &[&str] = &[
        "schema",
        "vault",
        "index",
        "curio",
        "zotero",
        "librarian",
        "mcp",
    ];
    const TABLES: &[(&str, &[&str])] = &[
        ("vault", &["path", "proposals_dir"]),
        (
            "index",
            &["path", "embedder", "chunk_tokens", "chunk_overlap"],
        ),
        ("curio", &["enabled", "events_dir", "notes_dirs"]),
        (
            "zotero",
            &[
                "enabled",
                "api_base",
                "user_id",
                "api_key_env",
                "webdav_fallback",
                "webdav_url",
            ],
        ),
        (
            "librarian",
            &["now_path", "digest_dir", "half_life_days", "top_k"],
        ),
        ("mcp", &["transport", "http_bind", "bearer_token_env"]),
    ];
    let mut unknown = Vec::new();
    for key in doc.keys() {
        if !TOP.contains(&key.as_str()) {
            unknown.push(key.clone());
        }
    }
    for (table, known) in TABLES {
        if let Some(toml::Value::Table(t)) = doc.get(*table) {
            for key in t.keys() {
                if !known.contains(&key.as_str()) {
                    unknown.push(format!("{table}.{key}"));
                }
            }
        }
    }
    Ok(unknown)
}

/// Expand a leading `~/` (or bare `~`) against `$HOME`. Non-tilde paths
/// pass through untouched; an unset `$HOME` leaves the tilde literal
/// rather than guessing.
#[must_use]
pub fn expand_tilde(path: &str) -> PathBuf {
    match std::env::var_os("HOME") {
        Some(home) => expand_tilde_with(path, Path::new(&home)),
        None => PathBuf::from(path),
    }
}

/// [`expand_tilde`] against an explicit home — the pure core, for tests.
#[must_use]
pub fn expand_tilde_with(path: &str, home: &Path) -> PathBuf {
    if path == "~" {
        home.to_owned()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(path)
    }
}

/// Resolve a secret named by an `*_env` config key: the config holds the
/// VARIABLE NAME, the environment holds the value. Empty names and unset
/// or empty variables resolve to `None`.
///
/// `KP_*` and `CURATOR_*` spellings alias each other: `CURATOR_<X>` is
/// the preferred spelling and wins when both are set, but the legacy
/// `KP_<X>` names (the kp-config/v1 defaults, e.g. `KP_ZOTERO_KEY`,
/// `KP_MCP_TOKEN`) remain honored — existing deployments keep working
/// unchanged.
#[must_use]
pub fn secret_from_env(var_name: &str) -> Option<String> {
    secret_with(var_name, |name| std::env::var(name).ok())
}

/// [`secret_from_env`] against an explicit lookup — the pure core, for tests.
pub fn secret_with(var_name: &str, get: impl Fn(&str) -> Option<String>) -> Option<String> {
    if var_name.is_empty() {
        return None;
    }
    let non_empty = |name: &str| get(name).filter(|v| !v.is_empty());
    // CURATOR_* is preferred; KP_* stays honored (in both directions, so a
    // config written against either naming resolves the other's variable).
    if let Some(rest) = var_name.strip_prefix("KP_") {
        return non_empty(&format!("CURATOR_{rest}")).or_else(|| non_empty(var_name));
    }
    if let Some(rest) = var_name.strip_prefix("CURATOR_") {
        return non_empty(var_name).or_else(|| non_empty(&format!("KP_{rest}")));
    }
    non_empty(var_name)
}

/// `[vault]` — the markdown corpus root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VaultConfig {
    /// Vault root directory (`~` expands to the home directory).
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
    /// Path of the `index.db` file (`~` expands to the home directory).
    pub path: String,
    /// `builtin` = in-process pinned CPU ONNX; `hash` = deterministic test embedder.
    pub embedder: String,
    /// Target chunk size, in tokens.
    pub chunk_tokens: u32,
    /// Overlap between adjacent chunks, in tokens.
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
    /// Whether the Curio producer is active.
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
    /// Whether the Zotero producer is active.
    pub enabled: bool,
    /// Zotero Web API base URL.
    pub api_base: String,
    /// Zotero numeric user id (the library to sync).
    pub user_id: String,
    /// Env var NAME holding the API key — never the key itself.
    pub api_key_env: String,
    /// Use the WebDAV `.prop`/`.zip` channel when the API lacks fulltext.
    pub webdav_fallback: bool,
    /// WebDAV base URL for the fallback channel.
    pub webdav_url: String,
}

impl ZoteroConfig {
    /// The API key, resolved through env indirection.
    #[must_use]
    pub fn api_key(&self) -> Option<String> {
        secret_from_env(&self.api_key_env)
    }
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
    /// Maximum number of entries per digest.
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
    /// Bind address for the streamable-HTTP transport.
    pub http_bind: String,
    /// Env var NAME holding the bearer token; required when transport = http.
    pub bearer_token_env: String,
}

impl McpConfig {
    /// The HTTP bearer token, resolved through env indirection.
    #[must_use]
    pub fn bearer_token(&self) -> Option<String> {
        secret_from_env(&self.bearer_token_env)
    }
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
    fn the_shipped_example_matches_the_contract_defaults() {
        // curator.example.toml at the repo root IS kp-config/v1's worked example;
        // this test pins it to the model (and the model's defaults) forever.
        let raw = include_str!("../../../curator.example.toml");
        let cfg = KpConfig::from_toml_str(raw).expect("example must parse");
        assert_eq!(cfg.schema, KP_CONFIG_SCHEMA);
        let defaults = KpConfig {
            schema: KP_CONFIG_SCHEMA.to_owned(),
            vault: VaultConfig::default(),
            index: IndexConfig::default(),
            curio: CurioConfig::default(),
            zotero: ZoteroConfig::default(),
            librarian: LibrarianConfig::default(),
            mcp: McpConfig::default(),
        };
        assert_eq!(cfg, defaults, "example file drifted from contract defaults");
        assert_eq!(
            unknown_keys(raw).expect("parses"),
            Vec::<String>::new(),
            "the shipped example must not itself trip unknown-key warnings"
        );
    }

    #[test]
    fn unknown_keys_never_fail_and_are_reported() {
        let raw = "schema = \"kp-config/v1\"\nfuture_key = true\n\n[vault]\npath = \"/tmp/v\"\nfrom_v2 = 3\n";
        let cfg = KpConfig::from_toml_str(raw).expect("unknown keys must not fail");
        assert_eq!(cfg.vault.path, "/tmp/v");
        assert_eq!(
            unknown_keys(raw).expect("parses"),
            vec!["future_key", "vault.from_v2"]
        );
    }

    #[test]
    fn unknown_schema_is_an_error() {
        let err = KpConfig::from_toml_str("schema = \"kp-config/v2\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::UnsupportedSchema { found } if found == "kp-config/v2"));
    }

    #[test]
    fn load_reads_a_file_and_expands_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("kp.toml");
        std::fs::write(
            &path,
            "schema = \"kp-config/v1\"\n[vault]\npath = \"/srv/vault\"\n",
        )
        .expect("write");
        let cfg = KpConfig::load(&path).expect("loads");
        assert_eq!(cfg.vault_path(), PathBuf::from("/srv/vault"));
    }

    #[test]
    fn load_missing_file_is_io_error() {
        let err = KpConfig::load("/nonexistent/kp.toml").unwrap_err();
        assert!(matches!(err, ConfigError::Io { .. }));
    }

    #[test]
    fn tilde_expansion() {
        let home = Path::new("/home/tester");
        assert_eq!(
            expand_tilde_with("~/vault", home),
            PathBuf::from("/home/tester/vault")
        );
        assert_eq!(expand_tilde_with("~", home), PathBuf::from("/home/tester"));
        // Only a LEADING tilde expands; these pass through verbatim.
        assert_eq!(
            expand_tilde_with("/abs/path", home),
            PathBuf::from("/abs/path")
        );
        assert_eq!(
            expand_tilde_with("rel/~/odd", home),
            PathBuf::from("rel/~/odd")
        );
        assert_eq!(
            expand_tilde_with("~user/vault", home),
            PathBuf::from("~user/vault")
        );
    }

    #[test]
    fn secret_indirection_reads_the_named_variable() {
        let env = |name: &str| (name == "KP_TEST_KEY").then(|| "s3cret".to_owned());
        assert_eq!(secret_with("KP_TEST_KEY", env), Some("s3cret".to_owned()));
        assert_eq!(secret_with("KP_OTHER", env), None);
        // Empty variable NAME (config left blank) resolves to nothing.
        assert_eq!(secret_with("", env), None);
        // Set-but-empty values are treated as unset.
        assert_eq!(secret_with("X", |_| Some(String::new())), None);
    }

    #[test]
    fn secret_from_env_unset_is_none() {
        assert_eq!(secret_from_env("KP_DEFINITELY_UNSET_VAR_XYZZY"), None);
    }

    #[test]
    fn curator_alias_is_preferred_for_kp_named_variables() {
        // Both spellings set: CURATOR_* wins.
        let both = |name: &str| match name {
            "CURATOR_ZOTERO_KEY" => Some("preferred".to_owned()),
            "KP_ZOTERO_KEY" => Some("legacy".to_owned()),
            _ => None,
        };
        assert_eq!(
            secret_with("KP_ZOTERO_KEY", both),
            Some("preferred".to_owned())
        );
        // Legacy spelling alone keeps working — deployments don't break.
        let legacy_only = |name: &str| (name == "KP_ZOTERO_KEY").then(|| "legacy".to_owned());
        assert_eq!(
            secret_with("KP_ZOTERO_KEY", legacy_only),
            Some("legacy".to_owned())
        );
        // An empty preferred value never shadows a set legacy one.
        let empty_preferred = |name: &str| match name {
            "CURATOR_ZOTERO_KEY" => Some(String::new()),
            "KP_ZOTERO_KEY" => Some("legacy".to_owned()),
            _ => None,
        };
        assert_eq!(
            secret_with("KP_ZOTERO_KEY", empty_preferred),
            Some("legacy".to_owned())
        );
    }

    #[test]
    fn curator_named_variables_fall_back_to_kp_spelling() {
        let legacy_only = |name: &str| (name == "KP_MCP_TOKEN").then(|| "tok".to_owned());
        assert_eq!(
            secret_with("CURATOR_MCP_TOKEN", legacy_only),
            Some("tok".to_owned())
        );
        let both = |name: &str| match name {
            "CURATOR_MCP_TOKEN" => Some("preferred".to_owned()),
            "KP_MCP_TOKEN" => Some("legacy".to_owned()),
            _ => None,
        };
        assert_eq!(
            secret_with("CURATOR_MCP_TOKEN", both),
            Some("preferred".to_owned())
        );
    }

    #[test]
    fn config_discovery_prefers_curator_spellings_and_honors_legacy() {
        let no_env = |_: &str| None;
        // $CURATOR_CONFIG wins over $KP_CONFIG and any file.
        let both_env = |name: &str| match name {
            "CURATOR_CONFIG" => Some("/a.toml".to_owned()),
            "KP_CONFIG" => Some("/b.toml".to_owned()),
            _ => None,
        };
        assert_eq!(
            discover_config(both_env, |_| true),
            (
                PathBuf::from("/a.toml"),
                ConfigSource::Env("CURATOR_CONFIG".to_owned())
            )
        );
        // $KP_CONFIG stays honored.
        let legacy_env = |name: &str| (name == "KP_CONFIG").then(|| "/b.toml".to_owned());
        assert_eq!(
            discover_config(legacy_env, |_| true),
            (
                PathBuf::from("/b.toml"),
                ConfigSource::Env("KP_CONFIG".to_owned())
            )
        );
        // ./curator.toml beats ./kp.toml when both exist.
        assert_eq!(
            discover_config(no_env, |_| true),
            (PathBuf::from("curator.toml"), ConfigSource::File)
        );
        // ./kp.toml alone is accepted (deprecated, callers warn).
        assert_eq!(
            discover_config(no_env, |name| name == "kp.toml"),
            (PathBuf::from("kp.toml"), ConfigSource::LegacyFile)
        );
        // Nothing found: default to the preferred name.
        assert_eq!(
            discover_config(no_env, |_| false),
            (PathBuf::from("curator.toml"), ConfigSource::Default)
        );
        // An empty env value is treated as unset.
        let empty_env = |_: &str| Some(String::new());
        assert_eq!(
            discover_config(empty_env, |_| false).1,
            ConfigSource::Default
        );
    }
}
