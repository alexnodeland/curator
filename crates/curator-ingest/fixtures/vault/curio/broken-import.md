---
schema: curio.frontmatter.v1
curio_id: not-a-uuid
title: "Broken import"
saved: whenever
tags: [broken]
---
This note claims curio.frontmatter.v1 but violates the schema
(bad curio_id, missing source/feed/published/checksum, bad saved).
It must be skipped with a warning, never crash ingest.
