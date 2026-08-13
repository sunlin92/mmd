import fs from 'node:fs';
import process from 'node:process';

export const UPDATER_ENDPOINT = 'https://github.com/sunlin92/mmd/releases/latest/download/latest.json';

const required = [
  'TAURI_SIGNING_PRIVATE_KEY',
  'TAURI_SIGNING_PRIVATE_KEY_PASSWORD',
  'TAURI_UPDATER_PUBLIC_KEY',
  'APPLE_CERTIFICATE',
  'APPLE_CERTIFICATE_PASSWORD',
  'APPLE_SIGNING_IDENTITY',
  'APPLE_ID',
  'APPLE_PASSWORD',
  'APPLE_TEAM_ID',
  'WINDOWS_CERTIFICATE_THUMBPRINT',
  'WINDOWS_CERTIFICATE_BASE64',
  'WINDOWS_CERTIFICATE_PASSWORD',
];

function invalid(name, value) {
  return !value?.trim() || value.trim() === '-' || /^(?:replace[-_ ]?me|placeholder|example)$/i.test(value.trim());
}

export function evaluateReleaseTrust(environment) {
  const errors = required
    .filter((name) => invalid(name, environment[name]))
    .map((name) => `${name} must contain a production trust value.`);
  return {
    errors,
    config: {
      bundle: {
        createUpdaterArtifacts: true,
        windows: { certificateThumbprint: environment.WINDOWS_CERTIFICATE_THUMBPRINT || '' },
      },
      plugins: {
        updater: {
          endpoints: [UPDATER_ENDPOINT],
          pubkey: environment.TAURI_UPDATER_PUBLIC_KEY || '',
        },
      },
    },
  };
}

function main() {
  const outputIndex = process.argv.indexOf('--output');
  const output = outputIndex >= 0 ? process.argv[outputIndex + 1] : null;
  const result = evaluateReleaseTrust(process.env);
  if (result.errors.length) {
    for (const error of result.errors) console.error(error);
    process.exitCode = 1;
    return;
  }
  if (output) fs.writeFileSync(output, `${JSON.stringify(result.config, null, 2)}\n`, { mode: 0o600 });
}

if (process.argv[1] && import.meta.url === new URL(process.argv[1], 'file:').href) main();
