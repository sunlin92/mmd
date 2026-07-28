# Releasing MMD

MMD uses a rolling public GitHub Release named **Latest**. Every push to `main` starts its own native builds and smoke tests. Only the final publishing jobs are serialized, so a newer push cannot interrupt a release mutation that has already started. Manual workflow dispatches are rehearsal-only and never create or modify a Release.

## Version and toolchains

Before merging a version change, update these four sources together:

- `package.json` and the root entries in `package-lock.json`
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`

Run `npm run check:release-version` to enforce the contract. CI uses Node 26.5.0 from `.node-version` and Rust 1.95.0 from `rust-toolchain.toml`.

## Verified assets

Four native GitHub-hosted runners build the release packages: `macos-15` arm64, `macos-15-intel` x64, `windows-latest` x64, and `ubuntu-22.04` x64. Ubuntu 22.04 is the Linux binary compatibility baseline.

Build jobs upload immutable workflow artifacts with an artifact ID, GitHub digest, and a strict file manifest. Fresh runners download those artifacts by ID, verify the API digest and manifest, then install and keep the application alive for at least five seconds. Only after all smoke jobs pass does a source-free job assemble:

- `MMD_<version>_aarch64.dmg`
- `MMD_<version>_x64.dmg`
- `MMD_<version>_x64-setup.exe`
- `MMD_<version>_amd64.AppImage`
- `MMD_<version>_amd64.deb`
- `SHA256SUMS.txt`

The publishing job first confirms that its source SHA is still the head of `main` and that no newer workflow generation for the same commit has started publishing. It then creates a unique candidate tag containing the workflow run ID, attempt, and source SHA. The job uploads a draft candidate, verifies the remote asset names, sizes, and downloaded SHA-256 values, and only then makes it public and marks it Latest. Any failure before that confirmed commit point triggers a retried, verified candidate rollback and restores the prior Latest marker.

Older workflow-managed Releases and tags in the `mmd-latest-*` namespace are deleted with idempotent retries. A successful run verifies that exactly one managed Release and one managed tag remain, both for the new candidate. Once deletion of the previous Release begins, the verified candidate is retained even if a final API read fails, avoiding a state with no downloadable Latest.

## Rehearsal and diagnosis

Run **Rolling Latest Release** manually to exercise source checks, all native builds, artifact identity checks, fresh install/start smoke jobs, and final asset assembly without running the write-scoped publishing job. Workflow artifacts are retained for seven days for diagnosis.

Branch pushes and pull requests run the same release-profile package builds through **Platform CI**. Its separate native smoke runners mount the DMGs, install and start the NSIS package, and install and start both Linux packages before a change reaches `main`.

The following evidence can only be obtained on GitHub-hosted native runners: DMG mounting and executable slice inspection, Windows NSIS install/uninstall behavior, WebView startup, and real FUSE AppImage mounting. A local YAML or build check cannot replace those jobs.

## Signing limitations

The baseline pipeline sets `APPLE_SIGNING_IDENTITY=-` so Tauri seals each macOS app bundle with a valid ad-hoc signature, and it accepts unsigned Windows Authenticode status. Ad-hoc signing is not Apple notarization, and an unsigned NSIS installer has no SmartScreen reputation. Users may still see Gatekeeper or SmartScreen warnings after downloading through a browser.

Before claiming frictionless platform trust, configure Apple Developer ID signing/notarization and Windows Authenticode signing in a protected environment, then repeat clean-device Safari and Edge download/install checks. Never expose signing credentials to pull-request or build jobs.
