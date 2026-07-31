import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import { baselineGateMarkdown, validateBaseline } from './baseline-schema.mjs';

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');

for (const corpus of ['10k', '100k']) {
  test(`${corpus} committed artifact is complete and documented value-for-value`, async () => {
    const [artifactText, documentation] = await Promise.all([
      readFile(path.join(projectRoot, 'scripts', 'perf', 'baselines', `${corpus}.json`), 'utf8'),
      readFile(path.join(projectRoot, 'docs', 'performance-baselines.md'), 'utf8'),
    ]);
    const artifact = JSON.parse(artifactText);

    assert.deepEqual(validateBaseline(artifact), []);
    assert.match(documentation, new RegExp(`^${baselineGateMarkdown(artifact)
      .replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}$`, 'm'));
  });
}

test('documents the exact FTS5 ADR trigger and memory measurement definition', async () => {
  const documentation = await readFile(
    path.join(projectRoot, 'docs', 'performance-baselines.md'),
    'utf8',
  );
  assert.match(documentation, /peak incremental memory.*maximum resident set size.*minus.*pre-build/si);
  assert.match(documentation, /100,000-file.*any.*build.*query.*memory.*cancellation.*FTS5 ADR/si);
  assert.match(documentation, /engineering acceptance gates.*not.*product.*promise/si);
});
