import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import test from 'node:test';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const repository = 'https://github.com/sunlin92/mmd';
const execFileAsync = promisify(execFile);

function cargoPackageValue(cargoToml, key) {
  const packageTable = cargoToml.match(/^\[package\]\s*$([\s\S]*?)(?=^\[|$(?![\s\S]))/m)?.[1] ?? '';
  return packageTable.match(new RegExp(`^${key}\\s*=\\s*"([^"]+)"\\s*$`, 'm'))?.[1];
}

test('publishes canonical Apache-2.0 project and manifest metadata', async () => {
  const [license, packageJsonText, cargoToml, tauriConfigText] = await Promise.all([
    readFile(path.join(projectRoot, 'LICENSE'), 'utf8'),
    readFile(path.join(projectRoot, 'package.json'), 'utf8'),
    readFile(path.join(projectRoot, 'src-tauri', 'Cargo.toml'), 'utf8'),
    readFile(path.join(projectRoot, 'src-tauri', 'tauri.conf.json'), 'utf8'),
  ]);
  const packageJson = JSON.parse(packageJsonText);

  assert.match(license, /Apache License\s+Version 2\.0, January 2004/);
  assert.match(license, /TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION/);
  assert.match(license, /END OF TERMS AND CONDITIONS/);
  assert.equal(
    createHash('sha256').update(license).digest('hex'),
    'cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30',
  );
  assert.equal(packageJson.private, true);
  assert.equal(packageJson.license, 'Apache-2.0');
  assert.equal(packageJson.repository, repository);
  assert.equal(cargoPackageValue(cargoToml, 'version'), packageJson.version);
  assert.equal(cargoPackageValue(cargoToml, 'license'), packageJson.license);
  assert.equal(cargoPackageValue(cargoToml, 'repository'), packageJson.repository);
  assert.deepEqual(JSON.parse(tauriConfigText).bundle.resources, { '../LICENSE': 'LICENSE' });
});

test('keeps the Tauri GUI as the explicit default binary when helper binaries exist', async () => {
  const manifestPath = path.join(projectRoot, 'src-tauri', 'Cargo.toml');
  const { stdout } = await execFileAsync(
    'cargo',
    ['metadata', '--manifest-path', manifestPath, '--no-deps', '--format-version', '1'],
    { cwd: projectRoot },
  );
  const metadata = JSON.parse(stdout);
  const mmdPackage = metadata.packages.find((candidate) => candidate.name === 'mmd');
  assert.ok(mmdPackage, 'Cargo metadata must contain the mmd package');
  assert.equal(mmdPackage.default_run, 'mmd');

  const binaryTargets = mmdPackage.targets.filter((target) => target.kind.includes('bin'));
  assert.deepEqual(
    binaryTargets.map((target) => target.name).sort(),
    ['mmd', 'mmd_bench'],
  );
  assert.equal(
    binaryTargets.find((target) => target.name === 'mmd')?.src_path,
    path.join(projectRoot, 'src-tauri', 'src', 'main.rs'),
  );
  assert.equal(
    binaryTargets.find((target) => target.name === 'mmd_bench')?.src_path,
    path.join(projectRoot, 'src-tauri', 'src', 'bin', 'mmd_bench.rs'),
  );
  assert.deepEqual(binaryTargets.find((target) => target.name === 'mmd')?.['required-features'] ?? [], []);
  assert.deepEqual(
    binaryTargets.find((target) => target.name === 'mmd_bench')?.['required-features'],
    ['bench-cli'],
  );
  assert.deepEqual(mmdPackage.features['bench-cli'], []);
  assert.deepEqual(mmdPackage.features.default, []);
});

test('documents shipped, experimental, and planned product state without stale gaps', async () => {
  const [readme, roadmap] = await Promise.all([
    readFile(path.join(projectRoot, 'README.md'), 'utf8'),
    readFile(path.join(projectRoot, 'ROADMAP.md'), 'utf8'),
  ]);

  for (const heading of ['## 已交付', '## 实验性能力', '## 计划路线']) {
    assert.match(readme, new RegExp(`^${heading.replaceAll('*', '\\*')}\\s*$`, 'm'));
  }
  assert.match(roadmap, /\[x\].*(五套|五种).*主题/);
  assert.match(roadmap, /\[x\].*(macOS|三平台).*CI/);
  assert.doesNotMatch(roadmap, /尚无三平台 CI/);
  assert.doesNotMatch(roadmap, /\[ \] 主题模式/);
});

test('uses only the documented roadmap status markers', async () => {
  const roadmap = await readFile(path.join(projectRoot, 'ROADMAP.md'), 'utf8');
  const markers = [...roadmap.matchAll(/^- \[([^\]]*)\]/gm)].map((match) => match[1]);
  assert.ok(markers.length > 0);
  assert.deepEqual([...new Set(markers)].sort(), [' ', 'x', '~']);
});
