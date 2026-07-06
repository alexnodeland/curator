---
title: "Hybrid retrieval: lexical + vector, fused"
created: 2026-05-14T09:20:00Z
updated: 2026-06-02T18:41:00Z
tags: [search, embeddings, ranking]
---

# Hybrid retrieval: lexical + vector, fused

Neither pure keyword search nor pure vector search wins on a personal
corpus. Keyword (FTS) search is precise on names, acronyms, and exact
phrases but blind to paraphrase; vector search recalls paraphrase and
adjacent ideas but hallucinates relevance on short queries.

The pragmatic answer is to run both and fuse. Reciprocal rank fusion
(RRF) is the boring, robust baseline: score each document by the sum of
`1 / (k + rank)` across result lists. No score normalization across
incomparable scales, no tuning beyond `k`, and it degrades gracefully
when one retriever returns garbage.

See [[embedded-vector-search]] for why the vector side can live inside
SQLite, and [[chunking-strategies]] for what actually gets embedded.
