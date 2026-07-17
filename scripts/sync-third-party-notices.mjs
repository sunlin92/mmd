import { createHash } from 'node:crypto';
import { copyFile, mkdir, readFile, readdir, rm } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const MAMMOTH_VERSION = '1.12.0';
export const DOMPURIFY_VERSION = '3.4.12';
export const EXCALIDRAW_VERSION = '0.18.1';
export const MERMAID_VERSION = '11.16.0';

export const NOTICE_SPECS = Object.freeze([
  Object.freeze({
    destination: 'dompurify/LICENSE',
    packageName: 'dompurify',
    sourceName: 'LICENSE',
    version: DOMPURIFY_VERSION,
  }),
  Object.freeze({
    destination: 'dompurify/LICENSE-MPL',
    packageName: 'dompurify',
    sourceName: 'LICENSE-MPL',
    version: DOMPURIFY_VERSION,
  }),
  Object.freeze({
    destination: 'mammoth/LICENSE',
    packageName: 'mammoth',
    sourceName: 'LICENSE',
    version: MAMMOTH_VERSION,
  }),
  // The published Excalidraw package contains only its distributable files.
  // Keep the upstream v0.18.1 license alongside this pinned integration.
  Object.freeze({
    destination: 'excalidraw/LICENSE',
    packageName: '@excalidraw/excalidraw',
    sourceName: 'LICENSE',
    sourcePath: 'scripts/licenses/excalidraw-0.18.1-LICENSE',
    version: EXCALIDRAW_VERSION,
  }),
  Object.freeze({
    destination: 'mermaid/LICENSE',
    packageName: 'mermaid',
    sourceName: 'LICENSE',
    version: MERMAID_VERSION,
  }),
]);

const projectRootFromModule = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '..',
);

function sha256Bytes(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function resolvePaths(projectRoot) {
  return {
    nodeModulesRoot: path.join(projectRoot, 'node_modules'),
    publicRoot: path.join(projectRoot, 'public', 'vendor', 'notices'),
  };
}

function noticeSourcePath(projectRoot, nodeModulesRoot, spec) {
  return spec.sourcePath
    ? path.join(projectRoot, spec.sourcePath)
    : path.join(nodeModulesRoot, spec.packageName, spec.sourceName);
}

async function collectRelativeFiles(directory, prefix = '') {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const relativePath = path.join(prefix, entry.name);
    const absolutePath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...await collectRelativeFiles(absolutePath, relativePath));
    } else if (entry.isFile()) {
      files.push(relativePath);
    }
  }
  return files.sort((left, right) => left.localeCompare(right));
}

async function assertPinnedPackageVersions(nodeModulesRoot) {
  const expectedVersions = new Map(NOTICE_SPECS.map((spec) => (
    [spec.packageName, spec.version]
  )));
  for (const [packageName, expectedVersion] of expectedVersions) {
    const packageJson = JSON.parse(await readFile(
      path.join(nodeModulesRoot, packageName, 'package.json'),
      'utf8',
    ));
    if (packageJson.version !== expectedVersion) {
      throw new Error(
        `Expected ${packageName} ${expectedVersion}, found ${String(packageJson.version)}`,
      );
    }
  }
}

export async function collectSynchronizedThirdPartyNotices({
  projectRoot = projectRootFromModule,
} = {}) {
  const { nodeModulesRoot, publicRoot } = resolvePaths(projectRoot);
  await assertPinnedPackageVersions(nodeModulesRoot);

  const expectedFiles = NOTICE_SPECS.map(({ destination }) => destination).sort();
  const actualFiles = await collectRelativeFiles(publicRoot);
  if (actualFiles.length !== expectedFiles.length
    || actualFiles.some((file, index) => file !== expectedFiles[index])) {
    throw new Error('Third-party notices are not synchronized');
  }

  const synchronized = [];
  for (const spec of NOTICE_SPECS) {
    const [sourceBytes, copiedBytes] = await Promise.all([
      readFile(noticeSourcePath(projectRoot, nodeModulesRoot, spec)),
      readFile(path.join(publicRoot, spec.destination)),
    ]);
    const sourceHash = sha256Bytes(sourceBytes);
    if (sourceHash !== sha256Bytes(copiedBytes)) {
      throw new Error(`Third-party notice hash mismatch: ${spec.destination}`);
    }
    synchronized.push({
      destination: spec.destination,
      packageName: spec.packageName,
      sha256: sourceHash,
      sourceName: spec.sourceName,
      sourcePath: spec.sourcePath ?? null,
    });
  }
  return synchronized;
}

export async function syncThirdPartyNotices({
  projectRoot = projectRootFromModule,
} = {}) {
  const { nodeModulesRoot, publicRoot } = resolvePaths(projectRoot);
  await assertPinnedPackageVersions(nodeModulesRoot);
  await rm(publicRoot, { force: true, recursive: true });

  for (const spec of NOTICE_SPECS) {
    const destination = path.join(publicRoot, spec.destination);
    await mkdir(path.dirname(destination), { recursive: true });
    await copyFile(
      noticeSourcePath(projectRoot, nodeModulesRoot, spec),
      destination,
    );
  }
  return collectSynchronizedThirdPartyNotices({ projectRoot });
}

const invokedPath = process.argv[1] ? path.resolve(process.argv[1]) : null;
if (invokedPath === fileURLToPath(import.meta.url)) {
  await syncThirdPartyNotices();
}
