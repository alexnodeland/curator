# Known issues — accepted gaps, with rationale

Findings from the 2026-07-03 adversarial review that were deliberately
NOT fixed (or only partially fixed). Each entry records the gap, why it
is accepted for v1, and what would close it. Everything the review
confirmed as a correctness/safety defect was fixed with a regression
test instead — this file is only the accepted remainder.

## 1. Builtin embedder identity is not pinned by model-artifact hash

The `builtin` embedder is pinned by name (`bge-small-en-v1.5`) and dims
(384) in the index meta. The downloaded ONNX artifact's hash is not
recorded or verified: a revised upstream file, a swapped/corrupt cache,
or a fastembed preprocessing change between library upgrades could
embed in a subtly different space while `check_meta` still passes —
mixed-space vectors with no error.

**Why accepted:** fastembed does not expose the artifact digest, and
hashing the cache directory ourselves would pin fastembed's internal
layout (a fragile coupling that breaks on their every release). The
practical trigger set is small: the model repo is versioned-immutable
by convention, and the cache lives next to the index (`.kp/models/`),
so wiping derived state wipes both together.

**Would close it:** record a model-file digest in the index meta at
first load (fail-fast on later mismatch), or fold the fastembed +
ort versions into the embedder id so library upgrades demand an epoch
rebuild. Revisit before any "bring your own model" feature.

## 2. Runtime identity parsing is laxer than the kp-note/v1 schema

`contracts/kp-note/v1.schema.json` pins shapes per namespace
(uuid-shaped `curio:`/`kp:`, `^[A-Z0-9]{8}$` for `zotero:`; the prose
says uuidv7). `KpId::from_str` accepts any non-empty identifier in a
known namespace, so hand-written notes with e.g. `kp_id: "kp:aaa"`
ingest, index, and round-trip unchallenged.

**Why accepted:** deliberate Postel split — the schema is normative for
*producers* (Curio, kp-zotero, and the digest writer all emit
schema-conformant ids, exercised by kp-core's conformance tests and the
librarian's uuid7 auto-apply gate), while the plane stays liberal in
what it *accepts* so a human's hand-minted note is never silently
dropped from search. Identity is an opaque key everywhere index-side;
a non-conforming id degrades nothing but the producer's own hygiene.

**Would close it:** shape validation at parse with a warn-not-reject
path in ingest, once there is a `kp doctor` rule to surface offenders
first (rejecting outright would evict existing notes from the index on
upgrade — worse than the disease).

## 3. `kp propose` has no staged-changes mode

The build spec's CLI line reads "create from staged changes or
generated content"; only the generated-content mode (`--from <dir>`)
exists. `contracts/proposals/v1.md` was amended to promise exactly what
ships and to name the staged-changes mode as planned work.

**Would close it:** a `kp propose --staged` that diffs working-tree
edits (vault under git) into a proposal. Needs a decision on how a
proposal-of-my-own-edits interacts with the apply validator's
clean-apply rule (the tree already contains the change).

## 4. Behavior counters can replay from backfilled files

Fixed for flags (`starred`/`read_later` are guarded by event ts — see
`apply_behavior`), but `opened_count` is a plain counter: if an
older-dated event file materializes after its events' dedupe rows were
pruned by the retention horizon, its `article.opened` events fold
again. Bounded by producer retention (≥90 days) and only reachable via
backfill/restore of pre-horizon files; a `kp reindex` no longer resets
dedupe state (consumer state carries across epochs), which removes the
common trigger.

**Would close it:** per-note high-water-mark of folded event ids, or
keying dedupe retention on the consumer's own horizon rather than the
oldest existing file.

## 5. The CI workflow has never executed on a hosted runner

`.github/workflows/ci.yml` mirrors `just ci` step for step, and
`just ci` runs green locally — but the repo has no remote until launch,
so the YAML itself (checkout, toolchain action, runner labels) is
unexercised. The workflow header says so.

**Would close it:** first push to a forge with Actions enabled; treat
the first green run as part of launch acceptance.
