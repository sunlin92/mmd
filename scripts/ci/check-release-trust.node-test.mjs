import assert from 'node:assert/strict';
import { test } from 'node:test';
import { evaluateReleaseTrust } from './check-release-trust.mjs';

const complete = {
  TAURI_SIGNING_PRIVATE_KEY: 'private-key',
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD: 'password',
  TAURI_UPDATER_PUBLIC_KEY: 'public-key',
  APPLE_CERTIFICATE: 'certificate',
  APPLE_CERTIFICATE_PASSWORD: 'certificate-password',
  APPLE_SIGNING_IDENTITY: 'Developer ID Application: MMD Release',
  APPLE_ID: 'release@mmd.invalid',
  APPLE_PASSWORD: 'app-password',
  APPLE_TEAM_ID: 'TEAM123',
  WINDOWS_CERTIFICATE_THUMBPRINT: '0123456789ABCDEF',
  WINDOWS_CERTIFICATE_BASE64: 'base64-pfx',
  WINDOWS_CERTIFICATE_PASSWORD: 'pfx-password',
};

test('accepts trusted release secrets and emits a production updater override', () => {
  const result = evaluateReleaseTrust(complete);
  assert.deepEqual(result.errors, []);
  assert.equal(result.config.bundle.createUpdaterArtifacts, true);
  assert.equal(result.config.plugins.updater.pubkey, 'public-key');
  assert.deepEqual(result.config.plugins.updater.endpoints, [
    'https://github.com/sunlin92/mmd/releases/latest/download/latest.json',
  ]);
});

test('rejects placeholder, ad-hoc, and incomplete trust configuration', () => {
  const result = evaluateReleaseTrust({
    ...complete,
    TAURI_UPDATER_PUBLIC_KEY: 'replace-me',
    APPLE_SIGNING_IDENTITY: '-',
    WINDOWS_CERTIFICATE_THUMBPRINT: '',
    WINDOWS_CERTIFICATE_BASE64: '',
  });
  assert.ok(result.errors.some((error) => error.includes('TAURI_UPDATER_PUBLIC_KEY')));
  assert.ok(result.errors.some((error) => error.includes('APPLE_SIGNING_IDENTITY')));
  assert.ok(result.errors.some((error) => error.includes('WINDOWS_CERTIFICATE_THUMBPRINT')));
});
