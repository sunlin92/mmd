import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const workflowPath = fileURLToPath(
  new URL('../.github/workflows/release.yml', import.meta.url),
);

test('source-free publisher gives every gh release command an explicit repository', async () => {
  const workflow = await readFile(workflowPath, 'utf8');
  const publisherStart = workflow.indexOf('\n  publish-latest:');

  assert.notEqual(publisherStart, -1, 'publish-latest job is missing');
  const publisher = workflow.slice(publisherStart);
  assert.doesNotMatch(publisher, /uses:\s*actions\/checkout@/);

  const releaseCommands = publisher
    .split('\n')
    .filter((line) => /\bgh release (?:create|delete|download|edit)\b/.test(line));

  assert.ok(releaseCommands.length > 0, 'publisher has no gh release commands');
  for (const command of releaseCommands) {
    assert.match(command, /--repo "\$GITHUB_REPOSITORY"/);
  }

  const createCommand = releaseCommands.find((command) => /\bgh release create\b/.test(command));
  assert.match(createCommand ?? '', /--verify-tag/);
});
