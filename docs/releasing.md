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

Build jobs upload immutable workflow artifacts with an artifact ID, GitHub digest, and a strict file manifest. Fresh runners download those artifacts by ID, verify the API digest and manifest, then install and keep the application alive for at least five seconds. Only after all smoke jobs pass does a read-only assembly job produce:

- `MMD_<version>_aarch64.dmg`
- `MMD_<version>_x64.dmg`
- `MMD_<version>_x64-setup.exe`
- `MMD_<version>_amd64.AppImage`
- `MMD_<version>_amd64.deb`
- signed Tauri updater archives and detached signatures for all four desktop targets
- `latest.json`, pointing at the immutable updater archive names in the Latest release
- `SHA256SUMS.txt`

The publishing job first confirms that its source SHA is still the head of `main` and that no newer workflow generation for the same commit has started publishing. It then creates a unique candidate tag containing the workflow run ID, attempt, and source SHA. The job uploads a draft candidate, verifies the remote asset names, sizes, and downloaded SHA-256 values, and only then makes it public and marks it Latest. Any failure before that confirmed commit point triggers a retried, verified candidate rollback and restores the prior Latest marker.

Older workflow-managed Releases and tags in the `mmd-latest-*` namespace are deleted with idempotent retries. A successful run verifies that exactly one managed Release and one managed tag remain, both for the new candidate. Once deletion of the previous Release begins, the verified candidate is retained even if a final API read fails, avoiding a state with no downloadable Latest.

## Rehearsal and diagnosis

Run **Rolling Latest Release** manually to exercise source checks, all native builds, artifact identity checks, fresh install/start smoke jobs, and final asset assembly without running the write-scoped publishing job. Workflow artifacts are retained for seven days for diagnosis.

Branch pushes and pull requests run the same release-profile package builds through **Platform CI**. Its separate native smoke runners mount the DMGs, install and start the NSIS package, and install and start both Linux packages before a change reaches `main`.

The following evidence can only be obtained on GitHub-hosted native runners: DMG mounting and executable slice inspection, Windows NSIS install/uninstall behavior, WebView startup, and real FUSE AppImage mounting. A local YAML or build check cannot replace those jobs.

## Trusted signing and updates

The rolling release workflow fails closed unless all production trust inputs are present. `scripts/ci/check-release-trust.mjs` validates the updater key inputs, Apple Developer ID/notarization inputs, and Windows Authenticode certificate inputs before any package build. It writes a temporary Tauri configuration override that enables updater artifacts, fixes the public `latest.json` endpoint, injects the updater public key, and pins the Windows certificate thumbprint. The override and decoded certificates are runner-local and are never uploaded.

Configure these protected GitHub Actions secrets:

- `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, `TAURI_UPDATER_PUBLIC_KEY`
- `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`
- `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`
- `WINDOWS_CERTIFICATE_BASE64`, `WINDOWS_CERTIFICATE_PASSWORD`, `WINDOWS_CERTIFICATE_THUMBPRINT`

The Windows build imports the PFX into the current-user certificate store and verifies the imported thumbprint before packaging. macOS builds use Tauri's Developer ID signing and notarization environment contract; ad-hoc identity `-` is explicitly rejected. Every updater archive must have a detached signature, and the assembly and publishing jobs compare exact asset allowlists before publishing.

The application checks for updates silently at startup. Network, endpoint, and runtime errors do not produce UI. A newer verified version opens a modal with Update, Later, and Skip This Version actions. A skipped version remains suppressed until a different version is published.

## Release smoke and rollback

After configuring or rotating any trust credential, run **Rolling Latest Release** with `workflow_dispatch`. This rehearsal builds, signs, installs, launches, verifies artifact identities, assembles `latest.json`, and uploads short-lived workflow artifacts without mutating GitHub Releases. Inspect macOS notarization output, Windows Authenticode status, all updater signatures, and the installed-app smoke evidence before allowing the next push to publish.

If a published update is defective, fix forward with a higher semantic version. The client and Tauri updater reject unsigned or metadata-mismatched packages, and the rolling publisher retains the previous Latest release until the new candidate is verified and promoted. A pre-promotion failure deletes the candidate and restores the prior Latest marker. Do not edit `latest.json` or replace an archive under an existing version; immutable names and signatures are part of the rollback contract.
