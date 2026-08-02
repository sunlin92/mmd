# Workspace Index Performance Baselines

These measurements are M3 engineering acceptance gates, not product or marketing promises. They apply only to the recorded operating system, architecture, build, production index identity, corpus identity, and configured limits. A comparison on a different environment or corpus requires an explicit rebaseline record.

## Reproduce

```bash
npm run perf:baseline:10k
npm run perf:baseline:100k
npm run test:perf
```

The two `perf:baseline:*` commands intentionally replace the committed frozen gate artifacts and are only for an explicit rebaseline. Routine M3 acceptance must use:

```bash
npm run perf:gate
```

That command regenerates the deterministic fixtures, writes fresh candidates and gate reports under `.perf/results/`, and never overwrites `scripts/perf/baselines/*.json`. To re-evaluate candidates already captured on the same machine, use `npm run perf:gate:check`.

The deterministic fixture generator uses fixture version `1` and seed `7417`. `run-baselines.mjs` reads those fixture files into `documents[{relativePath,content}]` and calls the release `cargo mmd_bench` binary. Search, build, and cancellation behavior all come from the production `workspace_index` module; the Node harness does not contain an alternate search implementation.

## Measurement Contract

- Cold build and warm query timings use `std::time::Instant` inside `mmd_bench`. Warmups run first and are excluded from every reported distribution.
- Warm query is the full-text query `deterministic markdown` against the just-built production in-memory index.
- Cancellation timing cancels that production full-text query after half of the corpus document checks.
- Peak incremental memory is a distribution from five independent `mmd_bench` processes. Each process reports its maximum resident set size from `getrusage` after its warmup plus one measured build/query sample minus that process's pre-build maximum resident set size; the artifact records all five raw deltas and aggregates them as `independentProcessPeakRssDelta`. The production core's estimated index bytes remain a separate field. Platforms without RSS measurement use the explicitly named `estimatedIndexBytesFallback` only for schema smoke and cannot establish a memory gate.
- Percentiles use nearest-rank selection. Errors and incomplete runs remain artifacts but cannot validate as baselines.

## Gate Evaluation

The evaluator exits zero only when both artifacts are complete, comparable, and every candidate p95 is less than or equal to its frozen p95. It exits nonzero for a threshold failure, an invalid/incomplete artifact, or a non-comparable candidate.

Comparability requires the same OS/architecture, production implementation ID, production schema ID, deterministic fixture/corpus identity, indexed byte count, configured limits, release runner, and measurement protocol. Peak-memory acceptance additionally requires `independentProcessPeakRssDelta`; `estimatedIndexBytesFallback` is valid only for harness smoke and cannot pass the frozen memory gate. Git commit and dirty state remain recorded evidence but do not change the frozen threshold contract.

Gate reports use three statuses:

- `pass`: all identity/protocol checks match and all four p95 thresholds pass.
- `fail`: artifacts are comparable, but one or more build/query/memory/cancellation thresholds exceed the frozen values.
- `not-comparable`: identity, corpus, limits, environment, or measurement protocol changed; this requires a matching-machine rerun or an explicit rebaseline decision, not a performance claim.

For a comparable 100,000-file result, `fts5AdrRequired` is true when any frozen build, query, memory, or cancellation threshold fails. Identity/protocol mismatches do not trigger that ADR because they provide no valid performance comparison.

## Frozen M3 Gates

All byte values are bytes and all timing values are milliseconds.

| Files | Indexed Markdown bytes | Max file bytes | Max aggregate bytes | Cold build p95 | Warm query p95 | Peak incremental memory p95 | Cancellation p95 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 10,000 | 3,572,792 | 1,048,576 | 268,435,456 | 87.44 | 19.659 | 16629760 | 6.176 |
| 100,000 | 35,969,134 | 1,048,576 | 268,435,456 | 602.721 | 108.578 | 156745728 | 56.385 |

## FTS5 ADR Trigger

For the deterministic 100,000-file corpus, failure of any frozen build, query, memory, or cancellation gate blocks M3 and requires an explicit `rusqlite`/FTS5 ADR. It does not authorize silently adding a persistent index, reducing corpus or file limits, truncating correct results, weakening authorization, or changing corpus identity without a recorded rebaseline.

The ADR must cover cache permissions, content privacy, corruption recovery, schema migration, authorization revocation and cleanup, and result equivalence with the production in-memory implementation.

## Artifacts

- `scripts/perf/baselines/10k.json`
- `scripts/perf/baselines/100k.json`

Each artifact records OS/architecture, app/build identity, production implementation/schema IDs, fixture and production corpus digests, configured limits, warmup/sample/timing metadata, distributions, memory details, errors, and incomplete/rebaseline state.
