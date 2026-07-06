---
title: "Embedded vector search — no server, one file"
created: 2026-05-16T21:03:00Z
tags: [search, sqlite, embeddings]
---

# Embedded vector search — no server, one file

A personal knowledge base does not need a vector database service. A
few thousand notes at 384 dimensions is a few megabytes of floats;
brute-force cosine over that is microseconds. The interesting
engineering is not ANN indexes, it is operational shape: one embedded
database file that lives next to the data it derives from, rebuilt from
scratch whenever anything doubts it.

SQLite extensions make this practical — vector similarity and FTS5
full-text search in the same database, one query surface, transactional
together. The derived index is disposable by construction: identity
lives in the notes, never in the database.

Related: [[hybrid-retrieval]], [[sqlite-as-application-file-format]].
