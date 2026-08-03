# M3 FTS5 Deferral Attestation

This directory preserves the three raw 100,000-file candidate reports and
their three gate reports used by ADR 0002. `attestation.json` fixes their
order and SHA-256 digests. Run `npm run verify:m3-attestation` from the
repository root to verify every digest, re-evaluate every gate against the
frozen baseline, enforce series comparability, and reproduce the decision.

The checked-in result is fail/pass/pass on one clean commit, so the series
decision is `defer`. This is a historical release-attestation integrity check.
CI does not rerun the timing benchmark, and a successful check is not fresh
performance evidence for the CI runner or for any supported platform.

## Provenance

- Candidate-producing commit: `b6b2335564eccf0616e84bf911ca363ce84b7100`
  (`test: reject ambiguous Windows evidence targets`). Each candidate embeds
  that full commit, `build.dirty: false`, and runner `cargo mmd_bench`.
- Capture context available from the source files: Darwin arm64, release
  profile, the `mmd-memory-substring-v1` implementation,
  `mmd-workspace-index-v1` schema, and the complete corpus, limits, and
  measurement identities stored in each candidate.
- Source directory at ADR commit `f5fb5c9e8b0fe49c004874da804706cd87ecfbb1`:
  `.omx/tmp/m3-fts5-research/evidence/`.
- The original shell command, operator/session identity, hardware model, and
  toolchain versions were not recorded in durable source material and are
  unavailable. They are intentionally not reconstructed or inferred here.
- The gate reports retain their original absolute `/tmp` candidate paths.
  Those path strings are historical metadata only; the attestation evaluator
  binds each checked-in gate report to its candidate by digest and by
  re-evaluating every gate check and decision field.
- Source file timestamps observed during preservation were
  `2026-08-02T20:36:38+0800` for all six files. Timestamps are contextual and
  are not used as integrity evidence.

The `ca41db5` A-side pair in the former temporary directory is not part of
this series: it records a different commit and is therefore not mutually
comparable under ADR 0002's exact-commit three-run rule.

## Digests

| Artifact | SHA-256 |
| --- | --- |
| `../../../../scripts/perf/baselines/100k.json` | `7beea421055b841c3dd58ef828c24a60de182be164b26a4f3912749b33cabfb0` |
| `mmd-m3-100k-b6b2335-run1.json` | `cba03c0b096ad7960bb6e112b357333bf9e7057dade15fc7b6cf3411608ff4b7` |
| `mmd-m3-100k-b6b2335-run1-gate.json` | `415fcfc29a1b03afa74b1ab37dfa0e03adc3f1393a911ebc135de744b86a3840` |
| `mmd-m3-100k-b6b2335-run2.json` | `b3a2565147dc17bd1adfe141654b9c5c993104f4b4fde1d040f75a4778959154` |
| `mmd-m3-100k-b6b2335-run2-gate.json` | `ffbee9e78fc9d5e0dcbff6fa84667add88980ed29c3930574e24fd337b557bfe` |
| `mmd-m3-100k-b6b2335-ab-b.json` | `f08166a53c1a01020f596734a4707a2ba8fb87baed51191ecb4796d0917e15da` |
| `mmd-m3-100k-b6b2335-ab-b-gate.json` | `5f99b6edaf6880d5b1d2179cadcfb1af123c39a2999b1aee76d7bbd50b7dda30` |

Platform-specific frozen baselines and three complete comparable runs on
macOS, Windows, and Linux remain prerequisites for any future FTS5 adoption.
This directory contains no such platform-specific adoption evidence.
