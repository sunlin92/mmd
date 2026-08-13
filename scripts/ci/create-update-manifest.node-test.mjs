import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createUpdateManifest } from './create-update-manifest.mjs';

test('creates deterministic updater metadata for all supported desktop targets', () => {
  const signatures = new Map([
    ['MMD_1.2.3_darwin-aarch64.app.tar.gz.sig', 'mac-arm-signature'],
    ['MMD_1.2.3_darwin-x86_64.app.tar.gz.sig', 'mac-x64-signature'],
    ['MMD_1.2.3_windows-x86_64.nsis.zip.sig', 'windows-signature'],
    ['MMD_1.2.3_linux-x86_64.AppImage.tar.gz.sig', 'linux-signature'],
  ]);
  const manifest = createUpdateManifest({
    version: '1.2.3',
    publishedAt: '2026-08-12T12:00:00.000Z',
    notes: 'Verified release',
    signatures,
  });

  assert.equal(manifest.version, '1.2.3');
  assert.equal(manifest.pub_date, '2026-08-12T12:00:00.000Z');
  assert.deepEqual(Object.keys(manifest.platforms), [
    'darwin-aarch64', 'darwin-x86_64', 'linux-x86_64', 'windows-x86_64',
  ]);
  assert.deepEqual(manifest.platforms['windows-x86_64'], {
    signature: 'windows-signature',
    url: 'https://github.com/sunlin92/mmd/releases/latest/download/MMD_1.2.3_windows-x86_64.nsis.zip',
  });
});

test('rejects missing, blank, or unexpected signatures', () => {
  const base = new Map([
    ['MMD_1.2.3_darwin-aarch64.app.tar.gz.sig', 'a'],
    ['MMD_1.2.3_darwin-x86_64.app.tar.gz.sig', 'b'],
    ['MMD_1.2.3_windows-x86_64.nsis.zip.sig', 'c'],
    ['MMD_1.2.3_linux-x86_64.AppImage.tar.gz.sig', 'd'],
  ]);
  base.delete('MMD_1.2.3_windows-x86_64.nsis.zip.sig');
  assert.throws(() => createUpdateManifest({ version: '1.2.3', publishedAt: '2026-08-12T12:00:00Z', notes: '', signatures: base }), /missing/);
  base.set('MMD_1.2.3_windows-x86_64.nsis.zip.sig', 'c');
  base.set('extra.sig', 'x');
  assert.throws(() => createUpdateManifest({ version: '1.2.3', publishedAt: '2026-08-12T12:00:00Z', notes: '', signatures: base }), /unexpected/);
});
