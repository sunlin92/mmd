# ADR 0001: Native OS Trash Adapter

- Status: Accepted
- Date: 2026-07-30
- Scope: M2 durable workspace deletion

## Context

The approved M2 baseline expected the Rust `trash` crate to be the sole new
Trash dependency. The implemented deletion contract is stricter than a
successful platform call: the application must classify every attempt as
confirmed committed, confirmed not committed, or indeterminate. A committed
result requires both a missing source and a recovery receipt whose destination
still contains the same filesystem object. The implementation must not fall
back to permanent deletion or to copy-then-delete.

The maintained `trash` 5.2.6 crate is healthy, MIT licensed, and compatible
with this project's Rust 1.95.0 toolchain. It is rejected for API semantics,
not project health:

- `delete` and `delete_all` return only `Result<(), Error>`, so callers receive
  neither the recovery destination nor a receipt that can be revalidated.
- The request to return the trashed path remains open as issue 38. Its proposed
  implementation, PR 109, remains unmerged and does not yet provide a complete
  cross-platform contract.
- Its macOS `NSFileManager` path passes no `resultingItemURL` output parameter.
- Its Windows path performs `IFileOperation::DeleteItem` without a progress
  sink, so it cannot retain the recycled `IShellItem` reported after deletion.
- Its Freedesktop implementation falls back to copy-then-delete on a
  cross-filesystem rename, which this application explicitly forbids.
- Its Linux implementation still documents a residual `getmntent` concurrency
  concern in open issue 42. OSV does not currently match an advisory to 5.2.6,
  but the upstream issue reinforces that adopting the crate would not remove
  all native Trash risk.

## Decision

Do not add `trash` 5.2.6. Keep the platform adapters behind the internal
`TrashPort` classification boundary and call the native capabilities needed to
produce and verify recovery receipts:

- Linux: create exact Freedesktop `.trashinfo` metadata, use no-replace rename,
  retain destination identity, and reject cross-filesystem copy/delete.
- macOS: call `NSFileManager.trashItem` with `resultingItemURL`, then verify the
  returned destination and filesystem identity.
- Windows: use an STA `IFileOperation` plus `IFileOperationProgressSink`, retain
  the recycled `IShellItem` from `PostDeleteItem`, and verify its path and file
  identity.

The direct dependencies are pinned and target-scoped where applicable:

| Package | Version | License | Declared MSRV | Purpose |
| --- | --- | --- | --- | --- |
| `libc` | 0.2.186 | MIT OR Apache-2.0 | 1.65 | Linux `renameat2` and Unix identity APIs |
| `objc2-foundation` | 0.3.2 | MIT | 1.71 | macOS `NSFileManager` and `NSURL` bindings |
| `windows` | 0.61.3 | MIT OR Apache-2.0 | 1.74 | Windows COM and Shell interfaces |
| `windows-core` | 0.61.2 | MIT OR Apache-2.0 | 1.74 | Windows COM implementation support |
| `windows-sys` | 0.61.2 | MIT OR Apache-2.0 | 1.71 | Handle-based Windows file identity checks |

These versions are compatible with the pinned Rust 1.95.0 toolchain and the
Apache-2.0 project license. They were already present at the same versions in
the Tauri dependency graph; declaring them directly exposes selected features
without adding parallel package versions. In contrast, `trash` 5.2.6 declares
Rust 1.85 and depends on `windows` 0.56, which would add a second Windows
bindings generation beside the existing 0.61 graph.

At acceptance time, version-specific OSV queries returned no matching
advisories for the five adopted versions or for `trash` 5.2.6. The historical
`windows` advisory RUSTSEC-2022-0008 affects versions before 0.32.0 and does not
affect 0.61.3.

## Consequences

Positive consequences:

- The three-state outcome is based on observable source and destination facts,
  rather than interpreting a unit success or an ambiguous platform error.
- Recovery receipts are available on all three supported desktop platforms.
- Linux cannot silently turn a Trash request into copy-then-permanent-delete.
- The selected binding versions align with the existing Tauri graph and have
  lower MSRVs than `trash` 5.2.6.

Negative consequences:

- The project owns security-sensitive native and unsafe adapter code instead of
  delegating platform behavior to one crate.
- OS API changes, Freedesktop Trash specification details, COM threading, and
  filesystem race handling remain local maintenance responsibilities.
- MIT notices must be retained for `objc2-foundation`; the dual-license notices
  for `libc`, `windows`, `windows-core`, and `windows-sys` must remain accurate.

The current notice synchronizer covers `libc`, `objc2-foundation`, `windows`,
and `windows-core`, but not the directly declared `windows-sys`. M2 must add and
hash-pin the applicable `windows-sys` upstream license text before release, or
document an equivalent distribution-level notice source that already covers it.

This is a dependency decision, not approval of every adapter implementation
detail. Native correctness remains subject to code review and platform evidence.

## Verification Requirements

The decision remains accepted only while all of these gates are maintained:

1. Unit tests cover the exact committed, not-committed, and indeterminate
   classifier, including unavailable, read-only, post-move, destination
   substitution, and recreated-source cases.
2. Real file and non-empty-directory Trash smoke tests run serially on macOS,
   Windows, and Linux CI.
3. Linux tests prove there is no copy/delete fallback and validate both the
   moved object identity and exact `.trashinfo` bytes.
4. macOS and Windows tests require a non-empty native recovery destination and
   revalidate the destination's file identity.
5. Windows source validation uses attributes and identity from a handle opened
   with reparse-point semantics before mutation.
6. Dependency updates rerun version-specific RustSec/OSV checks, target builds,
   native smoke tests, and license-notice synchronization.
7. License notice tests pin package names, versions, SPDX expressions, source
   texts, and SHA-256 hashes.

Reconsider this ADR if a released `trash` crate returns a trustworthy native
destination on every supported platform, exposes enough partial-failure state
for the three-state classifier, permits disabling Linux copy/delete, uses no
duplicated binding generation, and passes the same platform verification gates.

## Evidence

- `trash` crate metadata, releases, downloads, license, and MSRV:
  <https://crates.io/api/v1/crates/trash>
- `trash` 5.2.6 public API:
  <https://docs.rs/trash/5.2.6/trash/fn.delete.html>
- `trash` 5.2.6 dependency manifest:
  <https://docs.rs/crate/trash/5.2.6/source/Cargo.toml>
- Open destination-path request:
  <https://github.com/Byron/trash-rs/issues/38>
- Unmerged destination-return implementation:
  <https://github.com/Byron/trash-rs/pull/109>
- `trash` macOS implementation:
  <https://docs.rs/crate/trash/5.2.6/source/src/macos/mod.rs>
- `trash` Windows implementation:
  <https://docs.rs/crate/trash/5.2.6/source/src/windows.rs>
- `trash` Freedesktop copy/delete fallback:
  <https://docs.rs/crate/trash/5.2.6/source/src/freedesktop.rs>
- Open `getmntent` concurrency issue:
  <https://github.com/Byron/trash-rs/issues/42>
- Windows `PostDeleteItem` recovery-item contract:
  <https://learn.microsoft.com/windows/win32/api/shobjidl_core/nf-shobjidl_core-ifileoperationprogresssink-postdeleteitem>
- Apple `trashItem(at:resultingItemURL:)` contract:
  <https://developer.apple.com/documentation/foundation/filemanager/trashitem(at:resultingitemurl:)>
- Exact adopted crate metadata:
  <https://crates.io/crates/libc/0.2.186>,
  <https://crates.io/crates/objc2-foundation/0.3.2>,
  <https://crates.io/crates/windows/0.61.3>,
  <https://crates.io/crates/windows-core/0.61.2>, and
  <https://crates.io/crates/windows-sys/0.61.2>
- Historical, fixed Windows advisory:
  <https://rustsec.org/advisories/RUSTSEC-2022-0008.html>
