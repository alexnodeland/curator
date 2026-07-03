//! The chunker: fixed-size sliding token windows with overlap.
//!
//! Deliberately simple for v1 — whitespace tokens, joined back with single
//! spaces. The chunker version is part of the epoch function: changing
//! this algorithm means a new epoch, never an in-place migration.

/// Chunking parameters (from `[index]` in kp.toml).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkParams {
    /// Window size in tokens.
    pub tokens: usize,
    /// Tokens shared between consecutive windows.
    pub overlap: usize,
}

impl ChunkParams {
    /// Lift the contract config into chunk params.
    #[must_use]
    pub fn from_config(cfg: &kp_core::config::IndexConfig) -> Self {
        Self {
            tokens: cfg.chunk_tokens as usize,
            overlap: cfg.chunk_overlap as usize,
        }
    }
}

impl Default for ChunkParams {
    fn default() -> Self {
        // Mirrors the kp-config/v1 defaults.
        Self {
            tokens: 512,
            overlap: 64,
        }
    }
}

/// One chunk of a note body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// 0-based position within the note.
    pub ord: usize,
    pub text: String,
    /// Whitespace-token count of `text`.
    pub token_len: usize,
}

/// Split `text` into overlapping token windows. Empty/whitespace-only
/// input yields no chunks (title-only notes still reach FTS via the notes
/// table). Degenerate params are clamped: window >= 1, overlap < window.
#[must_use]
pub fn chunk_text(text: &str, params: ChunkParams) -> Vec<Chunk> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.is_empty() {
        return Vec::new();
    }
    let window = params.tokens.max(1);
    let step = window - params.overlap.min(window.saturating_sub(1));
    let mut chunks = Vec::new();
    let mut start = 0;
    loop {
        let end = (start + window).min(tokens.len());
        chunks.push(Chunk {
            ord: chunks.len(),
            text: tokens[start..end].join(" "),
            token_len: end - start,
        });
        if end == tokens.len() {
            break;
        }
        start += step;
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(tokens: usize, overlap: usize) -> ChunkParams {
        ChunkParams { tokens, overlap }
    }

    #[test]
    fn empty_and_whitespace_yield_no_chunks() {
        assert!(chunk_text("", params(8, 2)).is_empty());
        assert!(chunk_text("  \n\t ", params(8, 2)).is_empty());
    }

    #[test]
    fn short_text_is_one_chunk() {
        let chunks = chunk_text("just a few words", params(512, 64));
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "just a few words");
        assert_eq!(chunks[0].token_len, 4);
        assert_eq!(chunks[0].ord, 0);
    }

    #[test]
    fn windows_overlap_and_cover_everything() {
        // 10 tokens, window 4, overlap 1 -> step 3: [0..4) [3..7) [6..10)
        let text = "t0 t1 t2 t3 t4 t5 t6 t7 t8 t9";
        let chunks = chunk_text(text, params(4, 1));
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].text, "t0 t1 t2 t3");
        assert_eq!(chunks[1].text, "t3 t4 t5 t6");
        assert_eq!(chunks[2].text, "t6 t7 t8 t9");
        assert!(chunks.iter().all(|c| c.token_len == 4));
        assert_eq!(
            chunks.iter().map(|c| c.ord).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn tail_chunk_may_be_short() {
        let chunks = chunk_text("a b c d e", params(4, 0));
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[1].text, "e");
        assert_eq!(chunks[1].token_len, 1);
    }

    #[test]
    fn degenerate_params_are_clamped_not_infinite() {
        // overlap >= window would loop forever unclamped.
        let chunks = chunk_text("a b c d e f", params(2, 5));
        assert!(chunks.len() >= 3);
        assert_eq!(
            chunks.last().expect("nonempty").text.split(' ').next_back(),
            Some("f")
        );
        // window 0 clamps to 1.
        let chunks = chunk_text("x y", params(0, 0));
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn collapses_internal_whitespace() {
        let chunks = chunk_text("a\n\nb\t c", params(8, 0));
        assert_eq!(chunks[0].text, "a b c");
    }
}
