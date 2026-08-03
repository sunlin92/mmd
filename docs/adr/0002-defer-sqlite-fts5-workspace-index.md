# ADR 0002: Defer SQLite FTS5 Workspace Index

- Status: Accepted
- Date: 2026-08-03
- Scope: M3 workspace discovery and full-text search

## Context

M3 uses the production `mmd-memory-substring-v1` workspace index. The frozen
100,000-file performance contract requires an ADR whenever any comparable run
exceeds a baseline threshold. One exact-commit run exceeded the cold-build p95
threshold, so this decision is mandatory even though later runs passed.

The frozen cold-build p95 is 602.721 ms. On commit `b6b2335`, three complete,
comparable runs reported:

| Run | Cold-build p95 | Result |
| --- | ---: | --- |
| 1 | 621.794 ms | Fail, 19.073 ms / 3.16% above the threshold |
| 2 | 487.922 ms | Pass, 19.05% below the threshold |
| 3 | 528.084 ms | Pass, 12.38% below the threshold |

The median of these run-level p95 values is 528.084 ms. Warm query, peak
incremental memory, and cancellation passed in all three runs. The first
failure remains valid evidence and is not erased by the later passes, but the
series does not establish a sustained capacity failure.

The existing index contract is also broader than default FTS5 behavior. It
provides arbitrary-length Unicode substring matching, full case folding such
as `ss` matching sharp S, CJK substring matching, whitespace-term AND,
deterministic normalized-path and filename-rank ordering, bounded snippets,
and exact line and UTF-8 byte locations in the source. FTS5's closest built-in
option is the trigram tokenizer, but its full-text queries do not match
substrings shorter than three Unicode characters. Its query grammar, ranking,
and snippet behavior also differ.

## Decision

Defer `rusqlite` and SQLite FTS5. Keep `mmd-memory-substring-v1` as the M3
production implementation and add no database dependency in this milestone.

This decision treats the observed cold-build failure as variance requiring
triage, not proof that the current architecture cannot meet the frozen gate.
Every future comparable 100,000-file failure still blocks the affected
milestone or release until it is investigated and recorded.

Reopen dependency adoption when either condition is met:

1. On the same clean commit, corpus, limits, environment, and measurement
   contract, at least two of three consecutive fresh complete comparable runs
   fail the same frozen metric.
2. A newly approved corpus, limit, or correctness requirement cannot be met by
   the in-memory implementation without weakening result correctness,
   authorization, cancellation, or the approved resource bounds.

If reopened, evaluate a time-boxed prototype pinned as:

```toml
rusqlite = { version = "=0.40.1", default-features = false, features = ["bundled"] }
```

Do not use system SQLite for a production candidate. Its version and FTS5
compile options vary across the supported platforms. The bundled candidate
provides a controlled SQLite version with FTS5 enabled, but adoption still
requires three-platform measurement of binary size, build time, and runtime
performance.

## Cache Security and Privacy Requirements

The current baseline remains memory-only and creates no index files. A future
prototype may use SQLite `:memory:` without changing that baseline. It must not
use an empty-name temporary database as a privacy substitute because SQLite
may flush such a database to disk.

Any proposal for a persistent cache requires a separate approved ADR and all
of these properties:

- Store it only under the application cache directory, never in the workspace.
- Use owner-only directory and file permissions on Unix and a current-user
  restrictive ACL on Windows.
- Treat the cache as derived, non-authoritative, disposable, and rebuildable
  only from currently authorized Markdown.
- Bind it to an opaque workspace authorization identity and generation, index
  implementation/schema IDs, tokenizer and case-folding contract, corpus
  digest, and configured limits.
- Avoid raw content duplication where possible. Assume FTS terms can disclose
  document text and that default deletion does not reliably remove old terms.
- If persistent FTS5 is approved, enable both FTS5 `secure-delete=1` and core
  `PRAGMA secure_delete=1`; document that neither provides encryption or a
  physical-erasure guarantee.
- Include the main database plus `-wal`, `-shm`, `-journal`, and SQLite
  temporary artifacts in the permissions, cleanup, and threat model.

## Corruption, Migration, and Revocation

A future cache must never be used to recover user content. On open, schema, or
FTS5 `integrity-check` failure, close it, quarantine or delete the database and
all sidecars, and rebuild from currently authorized Markdown. Deletion failure
leaves the cache quarantined and unqueryable.

Known schema changes should normally discard and rebuild the complete cache.
Unknown future versions fail closed and are never queried. Publish a rebuilt
cache only after a complete build and a fresh authorization-generation check.

Authorization revocation must immediately cancel builders/readers, close all
connections, refuse results, and remove every database artifact. Cleanup
failure must not preserve serving authority. A later authorization grant builds
a new cache rather than reviving the old one.

## Semantic Equivalence Gate

FTS5 is eligible only as an accelerator; Markdown files remain authority. A
prototype must pass corpus-level differential tests against the production
implementation for:

- Exact normalized result paths, ordering, result limits, and truncation.
- Filename rank buckets and whitespace-term AND semantics.
- Unicode full case folding, CJK, sigma, sharp S, and arbitrary one- and
  two-character substring queries.
- Literal handling of input that FTS5 would otherwise parse as query syntax.
- Snippets, line numbers, and exact original-source UTF-8 byte offsets.
- Cancellation, configured resource limits, implementation/schema identity,
  discard/rebuild equivalence, and authorization-generation invalidation.

Adoption additionally requires all frozen 10,000- and 100,000-file build,
query, memory, and cancellation gates to pass on macOS, Windows, and Linux.

## Consequences

Positive consequences:

- M3 keeps the measured implementation that already satisfies two repeat
  gates and all non-build thresholds.
- No database, C compiler, migration, disk-cache, or new content-privacy
  surface is added.
- Search semantics remain identical to the tested production contract.
- The recorded failure produces a concrete, repeatable adoption trigger rather
  than being ignored or causing an unmeasured dependency change.

Negative consequences:

- The index is rebuilt after discard or restart and retains its current peak
  incremental memory cost.
- A persistent index cannot improve cold startup until a later ADR is approved.
- Comparable 100,000-file failures continue to block delivery even when the
  dependency-adoption threshold has not yet been reached.

If adopted later, bundled `rusqlite` adds `libsqlite3-sys`, a C compilation
step, binary size, build time, and responsibility for promptly updating the
embedded SQLite version. `rusqlite` is MIT and SQLite is public domain, both
compatible with this Apache-2.0 project, but notices and security scanning must
be updated at adoption time.

## Evidence

- Frozen 100,000-file baseline:
  `scripts/perf/baselines/100k.json`
- First comparable failure:
  `.omx/tmp/m3-fts5-research/evidence/mmd-m3-100k-b6b2335-run1-gate.json`
- First repeat pass:
  `.omx/tmp/m3-fts5-research/evidence/mmd-m3-100k-b6b2335-run2-gate.json`
- Second repeat pass:
  `.omx/tmp/m3-fts5-research/evidence/mmd-m3-100k-b6b2335-ab-b-gate.json`
- Production index implementation and equivalence tests:
  `src-tauri/src/workspace_index.rs`
- Authorization-scoped runtime invalidation:
  `src-tauri/src/workspace_index_runtime.rs`
- Authorization revalidation at the command boundary:
  `src-tauri/src/workspace_index_commands.rs`
- `rusqlite` 0.40.1 README and bundled guidance:
  <https://github.com/rusqlite/rusqlite/blob/v0.40.1/README.md>
- `libsqlite3-sys` build configuration:
  <https://github.com/rusqlite/rusqlite/blob/v0.40.1/libsqlite3-sys/build.rs>
- SQLite FTS5 and trigram tokenizer:
  <https://www.sqlite.org/fts5.html#the_trigram_tokenizer>
- FTS5 secure-delete, integrity-check, and rebuild:
  <https://www.sqlite.org/fts5.html#the_secure_delete_configuration_option>,
  <https://www.sqlite.org/fts5.html#the_integrity_check_command>, and
  <https://www.sqlite.org/fts5.html#the_rebuild_command>
- SQLite in-memory and temporary-file behavior:
  <https://www.sqlite.org/inmemorydb.html> and
  <https://www.sqlite.org/tempfiles.html>
- Package metadata and licenses:
  <https://crates.io/crates/rusqlite>,
  <https://github.com/rusqlite/rusqlite>, and
  <https://www.sqlite.org/copyright.html>
- Historical advisories outside the candidate version ranges:
  <https://rustsec.org/advisories/RUSTSEC-2020-0014.html>,
  <https://rustsec.org/advisories/RUSTSEC-2021-0128.html>, and
  <https://rustsec.org/advisories/RUSTSEC-2022-0090.html>
