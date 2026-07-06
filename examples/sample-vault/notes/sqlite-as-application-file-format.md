---
title: "SQLite as an application file format"
created: 2026-05-22T19:45:00Z
tags: [sqlite, architecture]
source: "https://sqlite.org/appfileformat.html"
---

# SQLite as an application file format

The SQLite team's own pitch: use a database file where you would have
invented a custom binary format. You get atomic writes, crash
recovery, incremental updates, a queryable catalog of your own state,
and a single file you can copy with `cp`.

For derived state — caches, indexes — the argument is even stronger:
if the file's schema ever fights an upgrade, delete it and rebuild.
A derived database that must be migrated is a design smell; a derived
database that can be deleted is an operational gift.
