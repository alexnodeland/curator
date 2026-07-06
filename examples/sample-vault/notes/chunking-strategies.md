---
title: "Chunking strategies for markdown notes"
created: 2026-05-20T08:12:00Z
tags: [embeddings, markdown]
---

# Chunking strategies for markdown notes

Embedding whole notes flattens long documents into mush; embedding
sentences loses context. For markdown the natural unit is the heading
section, split further only when a section overflows the model's
useful window, with a modest token overlap so ideas that straddle a
boundary still land in one chunk.

Two rules that survived contact with a real vault:

1. Never split inside a code fence — half a code block embeds as noise.
2. Keep the note title in every chunk's context; short chunks lose
   their subject otherwise.

The chunker's output is part of the index's identity: change the
chunking and every similarity subtly shifts, so a chunker change should
force a full rebuild rather than silently mixing regimes.
