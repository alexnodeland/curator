//! The `builtin` embedder: a pinned small CPU ONNX model, in-process.
//!
//! Behind the `embed-onnx` cargo feature (see docs/design/decisions.md
//! for the default-vs-opt-in ruling). The model is PINNED — id and dims
//! are compile-time constants recorded in every index epoch — and lazily
//! initialized: constructing a [`FastEmbedder`] does no I/O; the first
//! embed call loads (and on first ever use, fetches into `model_dir`) the
//! model. Tests never take that path — the hash embedder backs all
//! embedding tests.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use crate::embed::{EmbedError, Embedder};

/// The pinned model. Changing this constant IS an embedder change: every
/// existing index will (correctly) refuse to open and demand an epoch
/// rebuild.
pub const ONNX_MODEL_ID: &str = "bge-small-en-v1.5";
/// BGE-small-en-v1.5 output dimensionality.
pub const ONNX_DIMS: usize = 384;

/// In-process CPU ONNX embedder (`embedder = "builtin"` in kp.toml).
pub struct FastEmbedder {
    model_dir: PathBuf,
    /// Lazy: `None` until the first embed call. Mutex because ort sessions
    /// run with exclusive access; batch-CLI callers never contend.
    model: Mutex<Option<TextEmbedding>>,
}

impl std::fmt::Debug for FastEmbedder {
    // Manual: fastembed's TextEmbedding is not Debug.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let loaded = self.model.lock().map(|g| g.is_some()).unwrap_or(false);
        f.debug_struct("FastEmbedder")
            .field("model", &ONNX_MODEL_ID)
            .field("dims", &ONNX_DIMS)
            .field("model_dir", &self.model_dir)
            .field("loaded", &loaded)
            .finish()
    }
}

impl FastEmbedder {
    /// Create the embedder. No I/O happens here — the model directory is
    /// only touched on first use.
    #[must_use]
    pub fn new(model_dir: impl Into<PathBuf>) -> Self {
        Self {
            model_dir: model_dir.into(),
            model: Mutex::new(None),
        }
    }

    /// Conventional placement: models live next to the index database
    /// (`<index dir>/models/`), so wiping the derived-state directory
    /// wipes everything derived, together.
    #[must_use]
    pub fn from_config(config: &kp_core::KpConfig) -> Self {
        let index_path = config.index_path();
        let dir = index_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("models");
        Self::new(dir)
    }

    fn backend_err(e: impl std::fmt::Display) -> EmbedError {
        EmbedError::Backend {
            backend: ONNX_MODEL_ID.to_owned(),
            message: e.to_string(),
        }
    }
}

impl Embedder for FastEmbedder {
    fn id(&self) -> &str {
        ONNX_MODEL_ID
    }

    fn dims(&self) -> usize {
        ONNX_DIMS
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut guard = self
            .model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.is_none() {
            let model = TextEmbedding::try_new(
                InitOptions::new(EmbeddingModel::BGESmallENV15)
                    .with_cache_dir(self.model_dir.clone())
                    .with_show_download_progress(false),
            )
            .map_err(Self::backend_err)?;
            *guard = Some(model);
        }
        let model = guard.as_mut().expect("initialized above");
        let vectors = model.embed(texts, None).map_err(Self::backend_err)?;
        for v in &vectors {
            if v.len() != ONNX_DIMS {
                return Err(Self::backend_err(format!(
                    "model returned {} dims, pinned contract says {ONNX_DIMS}",
                    v.len()
                )));
            }
        }
        Ok(vectors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Hermetic tests only: constructing the embedder and reading its pinned
    // identity must do ZERO I/O (lazy init is the contract). Actual
    // inference needs a model on disk and is exercised manually / by the
    // reference instance, never by the suite.

    #[test]
    fn construction_is_lazy_and_does_no_io() {
        let dir = tempfile::tempdir().expect("tempdir");
        let model_dir = dir.path().join("models");
        let e = FastEmbedder::new(&model_dir);
        assert_eq!(e.id(), "bge-small-en-v1.5");
        assert_eq!(e.dims(), 384);
        assert!(
            !model_dir.exists(),
            "constructing must not touch the model dir"
        );
    }

    #[test]
    fn from_config_places_models_next_to_the_index() {
        let cfg = kp_core::KpConfig::from_toml_str(
            "schema = \"kp-config/v1\"\n[index]\npath = \"/tmp/kp-test/index.db\"\nembedder = \"builtin\"\n",
        )
        .expect("parses");
        let e = FastEmbedder::from_config(&cfg);
        assert_eq!(e.model_dir, std::path::Path::new("/tmp/kp-test/models"));
    }

    /// The real path — run manually with a network + `--ignored`:
    /// downloads the pinned model, embeds, checks dims and self-similarity.
    #[test]
    #[ignore = "downloads the pinned ONNX model; not part of the hermetic suite"]
    fn end_to_end_inference() {
        let dir = tempfile::tempdir().expect("tempdir");
        let e = FastEmbedder::new(dir.path().join("models"));
        let out = e
            .embed(&["hello world", "goodbye world"])
            .expect("inference");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].len(), ONNX_DIMS);
        let sim = crate::embed::cosine(&out[0], &out[1]);
        assert!(
            sim > 0.5 && sim < 0.999,
            "related-but-different texts: {sim}"
        );
    }
}
