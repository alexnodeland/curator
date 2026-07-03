//! The heading-aware markdown chunker.
//!
//! The generic token-window chunker in `kp-index` knows nothing about
//! markdown; this one does, and it is what ingest feeds the index with:
//!
//! - **headings start new chunks** — an ATX heading (`#`–`######`) always
//!   opens a fresh chunk and stays attached to the content below it;
//! - **code fences never split mid-fence** — a fenced block is atomic,
//!   even when it alone exceeds the token target;
//! - prose blocks accumulate up to `chunk_tokens`, and a single oversized
//!   prose block is split into token windows with `chunk_overlap` tokens
//!   shared between consecutive windows.
//!
//! Chunk text preserves the original lines (blocks re-joined with blank
//! lines), so snippets and embeddings see real markdown, not a
//! whitespace-flattened soup. Like the generic chunker, this algorithm is
//! part of the epoch function: changing it means a new epoch.

use kp_index::{Chunk, ChunkParams};

/// One structural block of a markdown document.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Block {
    /// An ATX heading line (`#`..`######`).
    Heading(String),
    /// A fenced code block, fences included. Atomic — never split.
    Fence(String),
    /// Anything else: one blank-line-separated run of prose lines.
    Text(String),
}

impl Block {
    fn text(&self) -> &str {
        match self {
            Block::Heading(s) | Block::Fence(s) | Block::Text(s) => s,
        }
    }

    fn tokens(&self) -> usize {
        self.text().split_whitespace().count()
    }
}

/// True for an ATX heading: 1–6 `#` followed by space/tab or end of line.
fn is_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return false;
    }
    matches!(trimmed[hashes..].chars().next(), None | Some(' ' | '\t'))
}

/// A fence opener: at least three backticks or tildes. Returns the fence
/// character and run length (the closer must use the same character, at
/// least as many, per CommonMark).
fn fence_open(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start();
    for c in ['`', '~'] {
        let run = trimmed.chars().take_while(|x| *x == c).count();
        if run >= 3 {
            return Some((c, run));
        }
    }
    None
}

fn fence_closes(line: &str, open: (char, usize)) -> bool {
    let trimmed = line.trim_start();
    let run = trimmed.chars().take_while(|x| *x == open.0).count();
    run >= open.1 && trimmed[run..].trim().is_empty()
}

/// Phase A: split markdown into heading / fence / text blocks.
fn blocks(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut prose: Vec<&str> = Vec::new();
    let mut fence: Option<((char, usize), Vec<&str>)> = None;

    let flush_prose = |prose: &mut Vec<&str>, out: &mut Vec<Block>| {
        if !prose.is_empty() {
            out.push(Block::Text(prose.join("\n")));
            prose.clear();
        }
    };

    for line in text.lines() {
        if let Some((open, lines)) = &mut fence {
            lines.push(line);
            if fence_closes(line, *open) {
                out.push(Block::Fence(lines.join("\n")));
                fence = None;
            }
            continue;
        }
        if let Some(open) = fence_open(line) {
            flush_prose(&mut prose, &mut out);
            fence = Some((open, vec![line]));
        } else if is_heading(line) {
            flush_prose(&mut prose, &mut out);
            out.push(Block::Heading(line.to_owned()));
        } else if line.trim().is_empty() {
            flush_prose(&mut prose, &mut out);
        } else {
            prose.push(line);
        }
    }
    flush_prose(&mut prose, &mut out);
    // An unterminated fence runs to end of input (CommonMark) — still atomic.
    if let Some((_, lines)) = fence {
        out.push(Block::Fence(lines.join("\n")));
    }
    out
}

/// Split one oversized prose block into token windows of `window` tokens,
/// consecutive windows sharing `overlap` tokens.
fn split_windows(text: &str, window: usize, overlap: usize) -> Vec<String> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let step = window - overlap.min(window.saturating_sub(1));
    let mut out = Vec::new();
    let mut start = 0;
    loop {
        let end = (start + window).min(tokens.len());
        out.push(tokens[start..end].join(" "));
        if end == tokens.len() {
            return out;
        }
        start += step;
    }
}

/// Chunk a markdown document (see module docs for the rules).
#[must_use]
pub fn chunk_markdown(text: &str, params: ChunkParams) -> Vec<Chunk> {
    let target = params.tokens.max(1);
    let overlap = params.overlap;

    let mut texts: Vec<String> = Vec::new();
    let mut current: Vec<&Block> = Vec::new();
    let mut current_tokens = 0usize;

    fn flush(current: &mut Vec<&Block>, current_tokens: &mut usize, texts: &mut Vec<String>) {
        if !current.is_empty() {
            texts.push(
                current
                    .iter()
                    .map(|b| b.text())
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            );
            current.clear();
            *current_tokens = 0;
        }
    }

    let doc_blocks = blocks(text);
    for block in &doc_blocks {
        let tokens = block.tokens();
        match block {
            // A heading always opens a fresh chunk.
            Block::Heading(_) => {
                flush(&mut current, &mut current_tokens, &mut texts);
                current.push(block);
                current_tokens = tokens;
            }
            // A fence is atomic: it joins the current chunk if it fits,
            // otherwise stands alone — but it is NEVER split.
            Block::Fence(_) => {
                if current_tokens + tokens > target && !current.is_empty() {
                    flush(&mut current, &mut current_tokens, &mut texts);
                }
                current.push(block);
                current_tokens += tokens;
                if current_tokens >= target {
                    flush(&mut current, &mut current_tokens, &mut texts);
                }
            }
            // Prose accumulates to the target; a single oversized block
            // becomes overlapping token windows.
            Block::Text(t) => {
                if tokens > target {
                    flush(&mut current, &mut current_tokens, &mut texts);
                    texts.extend(split_windows(t, target, overlap));
                    continue;
                }
                if current_tokens + tokens > target && !current.is_empty() {
                    flush(&mut current, &mut current_tokens, &mut texts);
                }
                current.push(block);
                current_tokens += tokens;
            }
        }
    }
    flush(&mut current, &mut current_tokens, &mut texts);

    texts
        .into_iter()
        .filter(|t| !t.trim().is_empty())
        .enumerate()
        .map(|(ord, text)| {
            let token_len = text.split_whitespace().count();
            Chunk {
                ord,
                text,
                token_len,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(tokens: usize, overlap: usize) -> ChunkParams {
        ChunkParams { tokens, overlap }
    }

    fn chunk_texts(md: &str, p: ChunkParams) -> Vec<String> {
        chunk_markdown(md, p).into_iter().map(|c| c.text).collect()
    }

    /// GOLDEN: a realistic small document — headings partition, prose
    /// accumulates, the fence rides with its section.
    #[test]
    fn golden_sectioned_document() {
        let md = "\
intro line one two three

# Alpha

alpha body four five six

## Beta

beta body seven eight

```rust
let x = 1;
```

# Gamma

gamma body nine ten
";
        let got = chunk_texts(md, params(64, 8));
        let want = vec![
            "intro line one two three".to_owned(),
            "# Alpha\n\nalpha body four five six".to_owned(),
            "## Beta\n\nbeta body seven eight\n\n```rust\nlet x = 1;\n```".to_owned(),
            "# Gamma\n\ngamma body nine ten".to_owned(),
        ];
        assert_eq!(got, want);
    }

    /// GOLDEN: prose accumulation respects the token target — paragraphs
    /// pack together until the next one would overflow.
    #[test]
    fn golden_prose_packs_to_target() {
        let md = "a1 a2 a3\n\nb1 b2 b3\n\nc1 c2 c3\n";
        // target 6: [a,b] fills exactly, c starts fresh.
        let got = chunk_texts(md, params(6, 0));
        assert_eq!(
            got,
            vec!["a1 a2 a3\n\nb1 b2 b3".to_owned(), "c1 c2 c3".to_owned()]
        );
    }

    /// GOLDEN: an oversized prose block splits into overlapping windows.
    #[test]
    fn golden_oversized_prose_windows_with_overlap() {
        let md = "t0 t1 t2 t3 t4 t5 t6 t7 t8 t9\n";
        let got = chunk_texts(md, params(4, 1));
        assert_eq!(
            got,
            vec![
                "t0 t1 t2 t3".to_owned(),
                "t3 t4 t5 t6".to_owned(),
                "t6 t7 t8 t9".to_owned(),
            ]
        );
    }

    /// GOLDEN: a fence longer than the target is one atomic chunk — never
    /// split mid-fence, no matter the budget.
    #[test]
    fn golden_code_fence_never_splits() {
        let md = "\
# Setup

```bash
cmd one two three four
cmd five six seven eight
cmd nine ten eleven twelve
```

after text
";
        let got = chunk_texts(md, params(5, 1));
        assert_eq!(
            got,
            vec![
                "# Setup".to_owned(),
                "```bash\ncmd one two three four\ncmd five six seven eight\ncmd nine ten eleven twelve\n```"
                    .to_owned(),
                "after text".to_owned(),
            ]
        );
        // No chunk anywhere contains a dangling fence delimiter.
        for text in &got {
            let fences = text.matches("```").count();
            assert_eq!(fences % 2, 0, "chunk splits a fence: {text:?}");
        }
    }

    /// Headings inside a fence are code, not structure.
    #[test]
    fn heading_inside_fence_does_not_split() {
        let md = "before\n\n```\n# not a heading\ntext\n```\n\nafter\n";
        let got = chunk_texts(md, params(64, 4));
        assert_eq!(
            got,
            vec!["before\n\n```\n# not a heading\ntext\n```\n\nafter".to_owned()]
        );
    }

    /// Every real heading starts its own chunk.
    #[test]
    fn every_heading_starts_a_chunk() {
        let md = "# One\n\nbody\n\n## Two\n\nbody\n\n### Three\n\nbody\n";
        let got = chunk_texts(md, params(512, 64));
        assert_eq!(got.len(), 3);
        assert!(got[0].starts_with("# One"));
        assert!(got[1].starts_with("## Two"));
        assert!(got[2].starts_with("### Three"));
    }

    #[test]
    fn tilde_fences_and_unterminated_fences_stay_atomic() {
        let md = "~~~\ncode a b c\n~~~\n\ntail\n";
        let got = chunk_texts(md, params(64, 0));
        assert_eq!(got, vec!["~~~\ncode a b c\n~~~\n\ntail".to_owned()]);
        // Unterminated fence runs to EOF, atomically.
        let md = "text\n\n```\nnever closed one two three four five six\n";
        let got = chunk_texts(md, params(3, 0));
        assert_eq!(
            got,
            vec![
                "text".to_owned(),
                "```\nnever closed one two three four five six".to_owned(),
            ]
        );
    }

    #[test]
    fn empty_and_whitespace_yield_no_chunks() {
        assert!(chunk_markdown("", params(8, 2)).is_empty());
        assert!(chunk_markdown("  \n\n\t \n", params(8, 2)).is_empty());
    }

    #[test]
    fn ord_and_token_len_are_consistent() {
        let md = "# H\n\none two three\n\n# H2\n\nfour five\n";
        let chunks = chunk_markdown(md, params(8, 2));
        for (i, c) in chunks.iter().enumerate() {
            assert_eq!(c.ord, i);
            assert_eq!(c.token_len, c.text.split_whitespace().count());
        }
    }

    #[test]
    fn not_headings() {
        assert!(!is_heading("####### seven hashes"));
        assert!(!is_heading("#hashtag"));
        assert!(is_heading("# real"));
        assert!(is_heading("###"));
        assert!(is_heading("  ## indented"));
    }

    #[test]
    fn degenerate_params_terminate() {
        // overlap >= window must clamp, not loop forever.
        let md = "a b c d e f g h\n";
        let got = chunk_texts(md, params(2, 5));
        assert!(got.len() >= 4);
        assert!(got.last().expect("nonempty").ends_with('h'));
        // window 0 clamps to 1.
        let got = chunk_texts("x y", params(0, 0));
        assert_eq!(got.len(), 2);
    }
}
