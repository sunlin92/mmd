import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  realpath,
  rm,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const script = fileURLToPath(new URL('./smoke-linux.sh', import.meta.url));

async function writeExecutable(file, contents) {
  await writeFile(file, `#!/bin/bash\nset -euo pipefail\n${contents}`);
  await chmod(file, 0o755);
}

test('installs a relative deb artifact through its absolute local path', async (t) => {
  const root = await mkdtemp(path.join(tmpdir(), 'mmd-smoke-linux-'));
  const binDir = path.join(root, 'bin');
  const artifactName = 'artifacts with spaces';
  const artifactDir = path.join(root, artifactName);
  const deb = path.join(artifactDir, 'MMD_0.1.0_amd64.deb');
  const sudoLog = path.join(root, 'sudo.log');
  t.after(() => rm(root, { recursive: true, force: true }));

  await mkdir(binDir);
  await mkdir(artifactDir);
  await writeFile(deb, 'fake deb');
  await writeFile(path.join(artifactDir, 'MMD_0.1.0_amd64.AppImage'), 'fake appimage');
  await writeExecutable(path.join(binDir, 'node'), 'exit 0\n');
  await writeExecutable(path.join(binDir, 'uname'), 'printf "x86_64\\n"\n');
  await writeExecutable(path.join(binDir, 'dpkg-deb'), 'exit 0\n');
  await writeExecutable(
    path.join(binDir, 'sudo'),
    `if [[ "$1" == apt-get && "$2" == update ]]; then
  exit 0
fi
printf '%s\\n' "$@" > "$SUDO_LOG"
exit 42
`,
  );

  const result = spawnSync('/bin/bash', [script, artifactName], {
    cwd: root,
    encoding: 'utf8',
    env: {
      ...process.env,
      CDPATH: root,
      PATH: `${binDir}:${process.env.PATH}`,
      SUDO_LOG: sudoLog,
    },
  });

  assert.equal(result.status, 42, `${result.stdout}\n${result.stderr}`);
  assert.deepEqual((await readFile(sudoLog, 'utf8')).trim().split('\n'), [
    'apt-get',
    'install',
    '-y',
    await realpath(deb),
  ]);
});
