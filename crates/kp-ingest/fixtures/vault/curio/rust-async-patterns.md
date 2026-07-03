---
schema: curio.frontmatter.v1
curio_id: 0197b2c4-8f3e-7cc1-a5d2-3e9f10aa4b6d
title: "Rust async patterns"
source: "https://example.com/rust-async-patterns"
feed: "https://example.com/feed.xml"
feed_title: "Example Blog"
author: "Jane Doe"
published: 2026-07-01T12:00:00Z
saved: 2026-07-01T08:30:00.123Z
tags: [rust, async]
checksum: "sha256:4a44dc15364204a80fe80e9039455cc1608281820fe2b24f1e5233ade6af1dd5"
lang: "en"
word_count: 87
---
<!-- curio:managed:begin v1 -->
# Rust async patterns

Structured concurrency beats fire-and-forget tasks. Cancellation is
cooperative: drop the future, and the work stops at the next await.

```rust
let handle = tokio::spawn(async move { fetch().await });
```
<!-- curio:managed:end -->

My companion notes: compare with [[notes/rust-notes|Rust notes]] — the
executor section overlaps.
