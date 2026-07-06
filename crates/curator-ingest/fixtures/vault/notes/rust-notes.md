---
kp_id: "kp:0197c001-2222-7bbb-8ccc-000000000001"
kp_schema: kp-note/v1
title: "Rust notes"
created: 2026-06-20T08:00:00Z
tags: [rust]
---
# Rust notes

Ownership, lifetimes, async.

## Async

Executors poll futures; see [[databases]] for storage-side notes
and [the chunking guide](guides/chunking.md).

```rust
async fn fetch() -> Result<(), Error> {
    Ok(())
}
```
