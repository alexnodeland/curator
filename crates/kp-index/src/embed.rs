//! The embedding seam.
//!
//! v1 ships two backends: `builtin` — a pinned, hash-verified small CPU
//! ONNX model running in-process (model id + dims recorded in the index
//! epoch) — and `hash` — a deterministic embedder that backs ALL embedding
//! tests, so the suite is hermetic: no network, no model downloads, no
//! external services. Mixed-model indexes are forbidden: a model id
//! mismatch triggers an epoch rebuild.

/// A text embedding backend.
pub trait Embedder {
    /// Stable backend identifier, recorded in the index epoch.
    fn model_id(&self) -> &str;
    /// Embedding dimensionality.
    fn dims(&self) -> usize;
    /// Embed one text into a vector of exactly `dims()` floats.
    fn embed(&self, text: &str) -> Vec<f32>;
}

/// The deterministic test embedder (`embedder = "hash"`).
///
/// Whitespace tokens are hashed (FNV-1a) into a per-token PRNG stream
/// (splitmix64) that scatters signed contributions across the vector; the
/// result is L2-normalized. Same input → same vector, forever, on every
/// platform, with zero I/O.
#[derive(Debug, Clone)]
pub struct HashEmbedder {
    dims: usize,
}

impl HashEmbedder {
    /// Default dimensionality for tests.
    pub const DEFAULT_DIMS: usize = 64;

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
    fn model_id(&self) -> &str {
        "hash"
    }

    fn dims(&self) -> usize {
        self.dims
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0_f32; self.dims];
        for token in text.split_whitespace() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_deterministic() {
        let e = HashEmbedder::default();
        assert_eq!(
            e.embed("rust databases sqlite"),
            e.embed("rust databases sqlite")
        );
    }

    #[test]
    fn distinct_inputs_differ() {
        let e = HashEmbedder::default();
        assert_ne!(
            e.embed("vectors and graphs"),
            e.embed("completely other text")
        );
    }

    #[test]
    fn respects_dims() {
        let e = HashEmbedder::new(17);
        assert_eq!(e.embed("anything").len(), 17);
        assert_eq!(e.dims(), 17);
    }

    #[test]
    fn empty_input_is_zero_vector() {
        let e = HashEmbedder::default();
        assert!(e.embed("").iter().all(|x| *x == 0.0));
    }

    #[test]
    fn nonempty_output_is_unit_norm() {
        let e = HashEmbedder::default();
        let v = e.embed("one two three");
        let norm = v
            .iter()
            .map(|x| f64::from(*x) * f64::from(*x))
            .sum::<f64>()
            .sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm was {norm}");
    }
}
