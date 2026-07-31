import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const runner = fileURLToPath(new URL('./packaged-lifecycle-runner.mjs', import.meta.url));

async function writeEvidence(evidencePath) {
  await writeFile(evidencePath, `${JSON.stringify({
    schema: 1,
    target: 'test-target',
    runId: '123',
    runAttempt: '2',
    commit: 'a'.repeat(40),
    packages: [{ name: 'MMD.test', size: 1, sha256: '1'.repeat(64) }],
    packageVariants: [{
      packageVariant: 'dmg',
      package: { name: 'MMD.test', size: 1, sha256: '1'.repeat(64) },
      packagedBinary: { name: 'mmd', size: 1, sha256: '2'.repeat(64), identity: { algorithm: 'file-sha256', sha256: '2'.repeat(64) } },
    }],
    scope: { packagedMutationE2e: false },
  })}\n`);
}

function runnerArguments(evidencePath, challengePath, command) {
  return [
    runner,
    '--evidence', evidencePath,
    '--package-variant', 'dmg',
    '--target', 'test-target',
    '--challenge-output', challengePath,
    '--', ...command,
  ];
}

async function removeChallengeRoot(challengePath) {
  const challenge = JSON.parse(await readFile(challengePath, 'utf8'));
  await rm(challenge.root, { recursive: true, force: true });
  return challenge;
}

test('coordinates stale CAS after ignoring an empty precreated receipt and preserves canonical environment', async (t) => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'mmd-packaged-runner-test-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const evidencePath = path.join(root, 'evidence.json');
  const challengePath = path.join(root, 'challenge.json');
  const reportPath = path.join(root, 'report.json');
  const termMarker = path.join(root, 'term-marker');
  await writeEvidence(evidencePath);
  const fakeApp = String.raw`
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const nonce = process.env.MMD_PACKAGED_LIFECYCLE_E2E_NONCE;
const root = path.join(os.tmpdir(), 'mmd-packaged-lifecycle-e2e', nonce);
const workspace = path.join(root, 'workspace');
fs.mkdirSync(workspace, { recursive: true });
fs.writeFileSync(path.join(workspace, 'save-stale.md'), 'initial bytes\n');
fs.writeFileSync(path.join(workspace, 'receipt.md'), '');
fs.writeFileSync(path.join(workspace, 'control.md'), 'ready\n');
process.on('SIGTERM', () => {
  fs.writeFileSync(process.env.TERM_MARKER, 'terminated');
  process.exit(9);
});
const timer = setInterval(() => {
  if (fs.readFileSync(path.join(workspace, 'control.md'), 'utf8') !== 'go\n') return;
  clearInterval(timer);
  fs.writeFileSync(process.env.REPORT_PATH, JSON.stringify({
    stale: fs.readFileSync(path.join(workspace, 'save-stale.md'), 'utf8'),
    runId: process.env.MMD_PACKAGED_LIFECYCLE_E2E_RUN_ID,
    runAttempt: process.env.MMD_PACKAGED_LIFECYCLE_E2E_RUN_ATTEMPT,
    commit: process.env.MMD_PACKAGED_LIFECYCLE_E2E_COMMIT,
    target: process.env.MMD_PACKAGED_LIFECYCLE_E2E_TARGET,
    variant: process.env.MMD_PACKAGED_LIFECYCLE_E2E_VARIANT,
  }));
  fs.writeFileSync(path.join(workspace, 'receipt.md'), JSON.stringify({ schema: 2, gate: 'packaged-lifecycle-e2e', status: 'passed' }) + '\n');
  setTimeout(() => process.exit(0), 100);
}, 10);
`;

  const result = spawnSync(process.execPath, runnerArguments(
    evidencePath,
    challengePath,
    [process.execPath, '-e', fakeApp],
  ), {
    encoding: 'utf8',
    env: {
      ...process.env,
      GITHUB_RUN_ID: '123',
      GITHUB_RUN_ATTEMPT: '2',
      GITHUB_SHA: 'a'.repeat(40),
      REPORT_PATH: reportPath,
      TERM_MARKER: termMarker,
    },
    timeout: 10_000,
  });

  assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
  const report = JSON.parse(await readFile(reportPath, 'utf8'));
  assert.deepEqual(report, {
    stale: 'external competing bytes\n',
    runId: '123',
    runAttempt: '2',
    commit: 'a'.repeat(40),
    target: 'test-target',
    variant: 'dmg',
  });
  await assert.rejects(readFile(termMarker, 'utf8'), { code: 'ENOENT' });
  await removeChallengeRoot(challengePath);
});

test('reports a packaged application signal exit without missing the exit event', async (t) => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'mmd-packaged-runner-signal-test-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const evidencePath = path.join(root, 'evidence.json');
  const challengePath = path.join(root, 'challenge.json');
  await writeEvidence(evidencePath);

  const result = spawnSync(process.execPath, runnerArguments(
    evidencePath,
    challengePath,
    [process.execPath, '-e', "process.kill(process.pid, 'SIGTERM')"],
  ), {
    encoding: 'utf8',
    env: {
      ...process.env,
      GITHUB_RUN_ID: '123',
      GITHUB_RUN_ATTEMPT: '2',
      GITHUB_SHA: 'a'.repeat(40),
      MMD_PACKAGED_LIFECYCLE_E2E_STOP_GRACE_MS: '25',
    },
    timeout: 5_000,
  });

  assert.equal(result.status, 1, `${result.stdout}\n${result.stderr}`);
  assert.match(result.stderr, /packaged application exited on signal SIGTERM before control ready receipt/);
  await removeChallengeRoot(challengePath);
});

test('reports packaged application spawn errors through the immediate completion observer', async (t) => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'mmd-packaged-runner-spawn-test-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const evidencePath = path.join(root, 'evidence.json');
  const challengePath = path.join(root, 'challenge.json');
  await writeEvidence(evidencePath);

  const result = spawnSync(process.execPath, runnerArguments(
    evidencePath,
    challengePath,
    [path.join(root, 'missing-packaged-application')],
  ), {
    encoding: 'utf8',
    env: {
      ...process.env,
      GITHUB_RUN_ID: '123',
      GITHUB_RUN_ATTEMPT: '2',
      GITHUB_SHA: 'a'.repeat(40),
    },
    timeout: 5_000,
  });

  assert.equal(result.status, 1, `${result.stdout}\n${result.stderr}`);
  assert.match(result.stderr, /packaged application failed to start: .*ENOENT.* before control ready receipt/);
  await removeChallengeRoot(challengePath);
});

test('terminates the packaged process tree and descendants after the graceful-close timeout', async (t) => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'mmd-packaged-runner-cleanup-test-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const evidencePath = path.join(root, 'evidence.json');
  const challengePath = path.join(root, 'challenge.json');
  const descendantPidPath = path.join(root, 'descendant.pid');
  await writeEvidence(evidencePath);
  const fakeApp = String.raw`
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { spawn } = require('node:child_process');
const root = path.join(os.tmpdir(), 'mmd-packaged-lifecycle-e2e', process.env.MMD_PACKAGED_LIFECYCLE_E2E_NONCE);
const workspace = path.join(root, 'workspace');
fs.mkdirSync(workspace, { recursive: true });
fs.writeFileSync(path.join(workspace, 'save-stale.md'), 'initial bytes\n');
fs.writeFileSync(path.join(workspace, 'receipt.md'), '');
fs.writeFileSync(path.join(workspace, 'control.md'), 'ready\n');
const timer = setInterval(() => {
  if (fs.readFileSync(path.join(workspace, 'control.md'), 'utf8') !== 'go\n') return;
  clearInterval(timer);
  const descendant = spawn(process.execPath, ['-e', "process.on('SIGTERM', () => {}); setInterval(() => {}, 1000)"], {
    stdio: 'ignore',
  });
  fs.writeFileSync(process.env.DESCENDANT_PID_PATH, String(descendant.pid));
  fs.writeFileSync(path.join(workspace, 'receipt.md'), JSON.stringify({ schema: 2, gate: 'packaged-lifecycle-e2e', status: 'passed' }) + '\n');
}, 10);
setInterval(() => {}, 1000);
`;

  const startedAt = Date.now();
  const result = spawnSync(process.execPath, runnerArguments(
    evidencePath,
    challengePath,
    [process.execPath, '-e', fakeApp],
  ), {
    encoding: 'utf8',
    env: {
      ...process.env,
      GITHUB_RUN_ID: '123',
      GITHUB_RUN_ATTEMPT: '2',
      GITHUB_SHA: 'a'.repeat(40),
      DESCENDANT_PID_PATH: descendantPidPath,
      MMD_PACKAGED_LIFECYCLE_E2E_STOP_GRACE_MS: '50',
      MMD_PACKAGED_LIFECYCLE_E2E_STOP_TERM_MS: '100',
      MMD_PACKAGED_LIFECYCLE_E2E_STOP_KILL_MS: '1000',
    },
    timeout: 5_000,
  });

  assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
  assert.ok(Date.now() - startedAt < 4_000, 'forced cleanup should use the short configured deadlines');
  const descendantPid = Number(await readFile(descendantPidPath, 'utf8'));
  assert.throws(() => process.kill(descendantPid, 0), { code: 'ESRCH' });
  await removeChallengeRoot(challengePath);
});
