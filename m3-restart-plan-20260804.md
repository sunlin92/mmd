# M3 Restart Plan

Recorded: 2026-08-04
Source goal: `G003-m3-native-open-discovery-and-input`
Current candidate: `284f8e407ba562422d8e58c6d124dc5bd6c8c01f`

## Stage Summary

- G001/M1 and G002/M2 are complete with durable external evidence.
- G003 product implementation and local verification are substantially green.
- G003's external completion gate is open: Platform CI `30875107319` passed
  frontend and native builds but failed all four installed-package smoke jobs;
  no verified artifacts exist.
- G004-G007 remain pending and must not start until the M3 gate closes.

## Old Goal Handling

G003 must remain audit-visible and must not be deleted or manually rewritten.
The intended Ultragoal mutation is:

```text
mark_blocked_superseded
target: G003-m3-native-open-discovery-and-input
replacement: M3 packaged smoke recovery and closeout
```

This mutation preserves G003's local-green and failed-CI evidence while making
the replacement story the only schedulable M3 work. The command was attempted
through `omx ultragoal steer` but rejected by the stale OMX 0.20.3 Ralplan
PreToolUse guard; no goals or ledger entries were changed.

## Replacement Story

### M3 packaged smoke recovery and closeout

1. Reproduce the sequential challenge/package-boundary startup failure as a
   focused RED test in the packaged runner/lifecycle harness.
2. Apply the smallest cleanup/readiness fix, preserving all platform profiles,
   timeouts, authorization checks, focus evidence, and package variants.
3. Run targeted Node/Vitest tests, then the full frontend and Rust verification
   suite.
4. Push a fresh exact SHA and require frontend, all four native builds, all four
   installed-package smoke jobs, and four validated `*-verified` artifacts.
5. Run independent code-reviewer and architect review plus `mmd-checkpoint`.
6. Checkpoint the replacement story and close M3 only after the external matrix
   and review gates are clean.

## Acceptance Gates

- Focused RED -> GREEN regression for sequential receiver cleanup/readiness.
- `npm test`, typecheck, lint, build, Rust fmt/test/check all pass.
- Platform CI exact-SHA matrix fully passes.
- Every verified artifact manifest and evidence file validates against the same
  run and commit.
- No weakened timeout, removed profile, skipped platform, or fabricated native
  evidence.

## Restart Sequence

1. Start a fresh Codex/OMX context without the stale `OMX_ROOT` session.
2. Execute the `mark_blocked_superseded` mutation through the Ultragoal CLI.
3. Clear the old aggregate `/goal` in the Codex UI as required by Ultragoal.
4. Start the replacement story through a fresh typed executor delegation.
5. Keep G004-G007 pending until M3 is checkpointed.

## Stop Condition

This document is a planning restart, not a completion checkpoint. The old goal
and ledger remain unchanged until a working Ultragoal control plane accepts the
replacement mutation and appends its audit event.
