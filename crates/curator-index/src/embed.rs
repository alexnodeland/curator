//! The embedding seam.
//!
//! v1 ships two backends: `builtin` — a pinned small CPU ONNX model
//! running in-process (see the `embed-onnx` cargo feature) — and `hash` —
//! a deterministic embedder that backs ALL embedding tests, so the suite
//! is hermetic: no network, no model downloads, no external services.
//!
//! The index epoch records the embedder id + dims; mixed-model indexes
//! are forbidden — an id/dims mismatch on open errors demanding an epoch
//! rebuild (enforced in [`crate::Index::open`]).

/// Errors from an embedding backend.
#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    /// The backend failed (model load, inference, ...).
    #[error("embedding backend {backend}: {message}")]
    Backend { backend: String, message: String },
}

/// A text embedding backend.
///
/// Batch-first: callers hand over every chunk of a note (or a whole
/// ingest batch) in one call so real backends can amortize inference.
pub trait Embedder: Send + Sync {
    /// Stable backend identifier (model id), recorded in the index epoch.
    fn id(&self) -> &str;
    /// Embedding dimensionality.
    fn dims(&self) -> usize;
    /// Embed a batch of texts into vectors of exactly [`Self::dims`] f32s,
    /// one per input, in input order.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError>;

    /// Convenience: embed a single text.
    fn embed_one(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        Ok(self
            .embed(&[text])?
            .pop()
            .expect("embed returns one vector per input"))
    }
}

impl std::fmt::Debug for dyn Embedder + '_ {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Embedder({}, dims={})", self.id(), self.dims())
    }
}

/// The deterministic test embedder (`embedder = "hash"`).
///
/// Whitespace tokens are hashed (FNV-1a) into a per-token PRNG stream
/// (splitmix64) that scatters signed contributions across the vector; the
/// result is L2-normalized. Same input → same vector, forever, on every
/// platform, with zero I/O. Token-sharing texts land measurably closer in
/// cosine space than disjoint ones — crude, but exactly enough semantics
/// for ranking tests to assert on.
#[derive(Debug, Clone)]
pub struct HashEmbedder {
    dims: usize,
}

impl HashEmbedder {
    /// Default dimensionality.
    pub const DEFAULT_DIMS: usize = 256;

    /// Create a hash embedder with the given dimensionality.
    #[must_use]
    pub fn new(dims: usize) -> Self {
        Self { dims }
    }
}

impl Default for HashEmbedder {
    fn default() -> Self {
        Self::new(Self::DEFAULT_DIMS)
    }
}

impl Embedder for HashEmbedder {
    fn id(&self) -> &str {
        "hash"
    }

    fn dims(&self) -> usize {
        self.dims
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(texts.iter().map(|t| self.embed_text(t)).collect())
    }
}

impl HashEmbedder {
    fn embed_text(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0_f32; self.dims];
        for token in text.split_whitespace() {
            // Case-fold and strip punctuation edges so "SQLite," and
            // "sqlite" contribute the same direction.
            let token = token.trim_matches(|c: char| !c.is_alphanumeric());
            if token.is_empty() {
                continue;
            }
            let token = token.to_lowercase();
            let mut state = fnv1a64(token.as_bytes());
            for slot in &mut v {
                let r = splitmix64(&mut state);
                // Map u64 → [-1, 1].
                #[allow(clippy::cast_precision_loss)]
                let unit = (r as f64) / (u64::MAX as f64) * 2.0 - 1.0;
                #[allow(clippy::cast_possible_truncation)]
                {
                    *slot += unit as f32;
                }
            }
        }
        let norm = v
            .iter()
            .map(|x| f64::from(*x) * f64::from(*x))
            .sum::<f64>()
            .sqrt();
        if norm > 0.0 {
            for slot in &mut v {
                #[allow(clippy::cast_possible_truncation)]
                {
                    *slot = (f64::from(*slot) / norm) as f32;
                }
            }
        }
        v
    }
}

/// FNV-1a 64-bit — stable across platforms and releases, dependency-free.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// splitmix64 — a tiny deterministic PRNG step.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Cosine similarity of two equal-length vectors (test + ranking helper).
#[must_use]
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let (mut dot, mut na, mut nb) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (x, y) in a.iter().zip(b) {
        dot += f64::from(*x) * f64::from(*y);
        na += f64::from(*x) * f64::from(*x);
        nb += f64::from(*y) * f64::from(*y);
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    #[allow(clippy::cast_possible_truncation)]
    {
        (dot / (na.sqrt() * nb.sqrt())) as f32
    }
}

/// The embedder named by `[index].embedder` in `kp.toml`: `"builtin"` —
/// the pinned in-process ONNX model (requires the `embed-onnx` or
/// `embed-onnx-dynamic` feature) —
/// or `"hash"`, the deterministic test embedder. The one selection point
/// every serving surface (CLI batch commands, MCP server) shares, so a
/// config can never mean two different models in two entrypoints.
pub fn embedder_from_config(
    config: &curator_core::KpConfig,
) -> Result<Box<dyn Embedder>, EmbedError> {
    match config.index.embedder.as_str() {
        "hash" => Ok(Box::new(HashEmbedder::default())),
        #[cfg(feature = "_embed-onnx-impl")]
        "builtin" => Ok(Box::new(crate::embed_onnx::FastEmbedder::from_config(
            config,
        ))),
        #[cfg(not(feature = "_embed-onnx-impl"))]
        "builtin" => Err(EmbedError::Backend {
            backend: "builtin".to_owned(),
            message: "this binary was built without ONNX support (no embed-onnx / \
                      embed-onnx-dynamic feature) — use embedder = \"hash\""
                .to_owned(),
        }),
        other => Err(EmbedError::Backend {
            backend: other.to_owned(),
            message: "unknown [index].embedder (expected \"builtin\" or \"hash\")".to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_deterministic() {
        let e = HashEmbedder::default();
        assert_eq!(
            e.embed(&["rust databases sqlite"]).expect("embeds"),
            e.embed(&["rust databases sqlite"]).expect("embeds")
        );
    }

    #[test]
    fn batch_preserves_order_and_matches_single() {
        let e = HashEmbedder::default();
        let batch = e.embed(&["alpha beta", "gamma delta"]).expect("embeds");
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0], e.embed_one("alpha beta").expect("embeds"));
        assert_eq!(batch[1], e.embed_one("gamma delta").expect("embeds"));
    }

    #[test]
    fn distinct_inputs_differ() {
        let e = HashEmbedder::default();
        assert_ne!(
            e.embed_one("vectors and graphs").expect("embeds"),
            e.embed_one("completely other text").expect("embeds")
        );
    }

    #[test]
    fn respects_dims() {
        let e = HashEmbedder::new(17);
        assert_eq!(e.embed_one("anything").expect("embeds").len(), 17);
        assert_eq!(e.dims(), 17);
        assert_eq!(HashEmbedder::default().dims(), 256);
    }

    #[test]
    fn empty_input_is_zero_vector() {
        let e = HashEmbedder::default();
        assert!(e.embed_one("").expect("embeds").iter().all(|x| *x == 0.0));
    }

    #[test]
    fn nonempty_output_is_unit_norm() {
        let e = HashEmbedder::default();
        let v = e.embed_one("one two three").expect("embeds");
        let norm = v
            .iter()
            .map(|x| f64::from(*x) * f64::from(*x))
            .sum::<f64>()
            .sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm was {norm}");
    }

    #[test]
    fn cosine_similarity_is_meaningful() {
        // The whole point of the hash embedder: overlapping-token texts
        // must be closer than disjoint ones, deterministically.
        let e = HashEmbedder::default();
        let db = e
            .embed_one("sqlite embedded database storage engine")
            .expect("embeds");
        let db2 = e
            .embed_one("embedded sqlite database index")
            .expect("embeds");
        let cooking = e.embed_one("pasta tomato basil olive oil").expect("embeds");
        assert!(cosine(&db, &db2) > cosine(&db, &cooking) + 0.2);
        // Case/punctuation folding: "SQLite," counts as "sqlite".
        let folded = e
            .embed_one("SQLite, EMBEDDED! database?? storage engine")
            .expect("embeds");
        assert!(cosine(&db, &folded) > 0.999);
    }
}
