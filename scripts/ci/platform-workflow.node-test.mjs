import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const workflowPath = fileURLToPath(
  new URL('../../.github/workflows/platform-ci.yml', import.meta.url),
);
const releaseWorkflowPath = fileURLToPath(new URL('../../.github/workflows/release.yml', import.meta.url));
const windowsSmokePath = fileURLToPath(new URL('./smoke-windows.ps1', import.meta.url));
const packagedRunnerPath = fileURLToPath(new URL('./packaged-lifecycle-runner.mjs', import.meta.url));
const nativeTrashPath = fileURLToPath(
  new URL('../../src-tauri/src/workspace_trash_native.rs', import.meta.url),
);
const smokePaths = [
  new URL('./smoke-macos.sh', import.meta.url),
  new URL('./smoke-linux.sh', import.meta.url),
  new URL('./smoke-windows.ps1', import.meta.url),
].map(fileURLToPath);

async function workflow() {
  return readFile(workflowPath, 'utf8');
}

async function smokes() {
  return (await Promise.all(smokePaths.map((file) => readFile(file, 'utf8')))).join('\n');
}

test('records target-native durable-write CAS and real Trash gates', async () => {
  const value = await workflow();

  assert.match(value, /run-cargo-tests\s+--gate durable-write-cas/);
  assert.match(value, /final_compare_race_retains_complete_competing_bytes/);
  assert.match(value, /expected_absent_install_race_retains_independent_intended_bytes/);
  assert.match(value, /staged_path_substitution_at_native_boundary_preserves_all_complete_images/);
  assert.match(value, /run-cargo-tests\s+--gate native-trash/);
  assert.match(value, /real_native_trash_round_trip_for_file_and_non_empty_directory/);
  assert.match(value, /MMD_RUN_NATIVE_TRASH_SMOKE:\s*'1'/);
});

test('verifies packaged main-binary identity on macOS Windows and Linux', async () => {
  const value = await workflow();

  assert.match(value, /Extract packaged macOS main binary/);
  assert.match(value, /Install and capture packaged Windows main binary/);
  assert.match(value, /Extract packaged Linux main binaries/);
  assert.match(value, /--source-binary/);
  assert.match(value, /--packaged-binary/);
  assert.match(value, /--identity-format macho-text/);
  assert.match(value, /--identity-format pe-text/);
  assert.match(value, /--identity-format elf-text/);
});

test('creates lifecycle evidence after package build and archives it in the manifest', async () => {
  const value = await workflow();
  const build = value.indexOf('- name: Build macOS package');
  const finalize = value.indexOf('- name: Finalize M2 lifecycle evidence');
  const manifest = value.indexOf('- name: Create artifact manifest');
  const upload = value.indexOf('uses: actions/upload-artifact@');

  assert.ok(build !== -1 && finalize > build, 'evidence must be finalized after packaging');
  assert.ok(manifest > finalize, 'artifact manifest must be created after lifecycle evidence');
  assert.ok(upload > manifest, 'artifact upload must follow manifest creation');
  assert.match(value, /m2-lifecycle-evidence\.json/);
  assert.match(value, /artifact-manifest\.mjs create staging/);
  assert.match(value, /artifact-manifest\.mjs verify staging/);
  assert.equal(value.match(/--receipt \.m2-evidence\/cas\.json/g)?.length, 3);
  assert.equal(value.match(/--receipt \.m2-evidence\/trash\.json/g)?.length, 3);
});

test('builds instrumented packages and promotes only post-smoke verified artifacts', async () => {
  const value = await workflow();
  const smoke = await smokes();

  assert.match(value, /VITE_MMD_PACKAGED_LIFECYCLE_E2E:\s*'1'/);
  assert.equal(
    value.match(/npm run tauri -- build[^\n]+--features packaged-lifecycle-e2e/g)?.length,
    2,
  );
  assert.match(smoke, /packaged-lifecycle-runner\.mjs/);
  assert.match(smoke, /verify-packaged/);
  assert.match(value, /name: \$\{\{ matrix\.artifact \}\}-base/);
  assert.match(value, /name: \$\{\{ matrix\.artifact \}\}-verified/);
  assert.match(value, /m2-lifecycle-evidence\.json/);
  assert.ok(
    value.lastIndexOf('uses: actions/upload-artifact@') > value.indexOf('smoke-linux.sh'),
    'verified artifact must be uploaded after packaged smoke verification',
  );
});

test('runs packaged lifecycle feature tests on every native target before packaging', async () => {
  const value = await workflow();
  const featureTest =
    'cargo test --manifest-path src-tauri/Cargo.toml --target ${{ matrix.target }} --release --features packaged-lifecycle-e2e packaged_lifecycle_e2e::tests';
  const featureTestIndex = value.indexOf(featureTest);
  const macosBuildIndex = value.indexOf('- name: Build macOS package');
  const nonMacosBuildIndex = value.indexOf('- name: Build non-macOS package');

  assert.equal(value.split(featureTest).length - 1, 1);
  assert.ok(featureTestIndex !== -1, 'native matrix must run the packaged lifecycle feature tests');
  assert.ok(featureTestIndex < macosBuildIndex, 'feature tests must run before the macOS package build');
  assert.ok(
    featureTestIndex < nonMacosBuildIndex,
    'feature tests must run before the non-macOS package build',
  );
});

test('keeps the CI-only instrumentation out of default release packages', async () => {
  const release = await readFile(releaseWorkflowPath, 'utf8');

  assert.doesNotMatch(release, /packaged-lifecycle-e2e/);
  assert.doesNotMatch(release, /VITE_MMD_PACKAGED_LIFECYCLE_E2E/);
});

test('passes canonical workflow identity to installed packages and runs both Linux formats', async () => {
  const value = await workflow();

  assert.match(value, /GITHUB_RUN_ID: \$\{\{ github\.run_id \}\}/);
  assert.match(value, /GITHUB_RUN_ATTEMPT: \$\{\{ github\.run_attempt \}\}/);
  assert.match(value, /GITHUB_SHA: \$\{\{ github\.sha \}\}/);
  assert.match(value, /smoke-macos\.sh artifact \$\{\{ matrix\.expected_arch \}\} \$\{\{ matrix\.target \}\}/);
  assert.match(value, /smoke-windows\.ps1 -ArtifactDirectory artifact -Target \$\{\{ matrix\.target \}\}/);
  assert.match(value, /smoke-linux\.sh artifact \$\{\{ matrix\.target \}\}/);
  assert.match(value, /MMD_ci_amd64\.deb/);
  assert.match(value, /MMD_ci_amd64\.AppImage/);
});

test('uses the Windows user-local temp volume for packaged Trash fixtures', async () => {
  const value = await readFile(windowsSmokePath, 'utf8');
  const runner = await readFile(packagedRunnerPath, 'utf8');
  const selectLocalTemp = value.indexOf(
    '[Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)',
  );
  const assignLifecycleTemp = value.indexOf(
    "$lifecycleTemp = Join-Path $localAppData 'Temp'",
  );
  const compareSystemVolume = value.indexOf('if ($lifecycleVolume -ine $systemVolume)');
  const createLifecycleTemp = value.indexOf(
    'New-Item -ItemType Directory -Path $lifecycleTemp -Force',
  );
  const runPackagedLifecycle = value.indexOf('packaged-lifecycle-runner.mjs');

  assert.notEqual(selectLocalTemp, -1, 'Windows smoke must select LocalApplicationData');
  assert.notEqual(assignLifecycleTemp, -1, 'Windows smoke must use LocalApplicationData\\Temp');
  assert.notEqual(compareSystemVolume, -1, 'Windows smoke must enforce the system volume');
  assert.notEqual(createLifecycleTemp, -1, 'Windows smoke must create the lifecycle temp');
  assert.ok(
    selectLocalTemp < assignLifecycleTemp
      && assignLifecycleTemp < compareSystemVolume
      && compareSystemVolume < createLifecycleTemp
      && createLifecycleTemp < runPackagedLifecycle,
    'the system-volume lifecycle temp must be prepared before the packaged app starts',
  );
  assert.match(value, /\$systemVolume = \[IO\.Path\]::GetPathRoot\(\[Environment\]::SystemDirectory\)/);
  assert.match(value, /\$lifecycleVolume = \[IO\.Path\]::GetPathRoot\(\$lifecycleTemp\)/);
  assert.match(value, /\$env:TEMP = \$lifecycleTemp/);
  assert.match(value, /\$env:TMP = \$lifecycleTemp/);
  assert.match(value, /\$env:TMPDIR = \$lifecycleTemp/);
  assert.match(runner, /Packaged lifecycle challenge root:/);
});

test('keeps raw Windows Trash diagnostics behind packaged lifecycle instrumentation', async () => {
  const value = await readFile(nativeTrashPath, 'utf8');
  const gatedDiagnostics = [
    '        #[cfg(feature = "packaged-lifecycle-e2e")]',
    '        match &result {',
    '            MoveToTrash::Rejected { error } => {',
    '                eprintln!("Packaged lifecycle Windows Trash rejected: {error}");',
    '            }',
    '            MoveToTrash::PossiblyMoved { error, .. } => {',
    '                eprintln!("Packaged lifecycle Windows Trash was not proven: {error}");',
    '            }',
    '            MoveToTrash::Placed { .. } => {}',
    '        }',
    '        result',
    '    }',
  ].join('\n');

  assert.ok(
    value.includes(gatedDiagnostics),
    'raw Windows Trash diagnostics must remain inside the packaged lifecycle feature gate',
  );
});
