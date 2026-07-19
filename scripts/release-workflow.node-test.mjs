import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const workflowPath = fileURLToPath(
  new URL('../.github/workflows/release.yml', import.meta.url),
);

async function readPublisher() {
  const workflow = await readFile(workflowPath, 'utf8');
  const publisherStart = workflow.indexOf('\n  publish-latest:');

  assert.notEqual(publisherStart, -1, 'publish-latest job is missing');
  return workflow.slice(publisherStart);
}

async function readPublishScript() {
  const publisher = await readPublisher();
  const step = publisher.indexOf('      - name: Create, verify, and roll Latest\n');
  assert.notEqual(step, -1, 'publish transaction step is missing');

  const runMarker = '        run: |\n';
  const scriptStart = publisher.indexOf(runMarker, step);
  assert.notEqual(scriptStart, -1, 'publish transaction script is missing');

  const script = [];
  for (const line of publisher.slice(scriptStart + runMarker.length).split('\n')) {
    if (line.startsWith('          ')) {
      script.push(line.slice(10));
    } else if (line === '') {
      script.push('');
    } else {
      break;
    }
  }
  return `${script.join('\n')}\n`;
}

function readShellFunction(publisher, name) {
  const marker = `          ${name}() {\n`;
  const start = publisher.indexOf(marker);
  assert.notEqual(start, -1, `${name} function is missing`);

  const end = publisher.indexOf('\n          }\n', start);
  assert.notEqual(end, -1, `${name} function is unterminated`);
  return publisher.slice(start, end + '\n          }'.length);
}

test('source-free publisher gives every gh release command an explicit repository', async () => {
  const publisher = await readPublisher();
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

test('draft candidate metadata is read by release ID instead of the published-tag endpoint', async () => {
  const publisher = await readPublisher();
  const resolver = readShellFunction(publisher, 'find_candidate_release_id');
  const rollback = readShellFunction(publisher, 'delete_candidate_release_or_confirm_absent');

  assert.doesNotMatch(
    publisher,
    /repos\/\$GITHUB_REPOSITORY\/releases\/tags\/\$tag\b/,
  );
  assert.match(publisher, /candidate_release_id=\$\(retry find_candidate_release_id\)/);
  assert.match(resolver, /gh api --paginate "repos\/\$GITHUB_REPOSITORY\/releases\?per_page=100" --slurp/);
  assert.match(resolver, /select\(\.tag_name == \$tag\)/);
  assert.match(resolver, /if length == 1 then \.\[0\]\.id/);
  assert.match(
    publisher,
    /candidate_draft=\$\(gh api "repos\/\$GITHUB_REPOSITORY\/releases\/\$candidate_release_id" --jq \.draft\)/,
  );
  assert.match(
    publisher,
    /gh api -X DELETE "repos\/\$GITHUB_REPOSITORY\/releases\/\$candidate_release_id"/,
  );
  assert.match(
    publisher,
    /retry gh api -X PATCH "repos\/\$GITHUB_REPOSITORY\/releases\/\$candidate_release_id" -F draft=false -f make_latest=true/,
  );
  assert.match(rollback, /if \[\[ "\$candidate_release_id" =~ \^\[0-9\]\+\$ \]\]/);
  const idDelete = rollback.indexOf('gh api -X DELETE "repos/$GITHUB_REPOSITORY/releases/$candidate_release_id"');
  const tagFallback = rollback.indexOf('gh release delete "$tag"');
  assert.ok(idDelete !== -1 && tagFallback > idDelete);
  assert.match(rollback, /fi\n            candidate_release_absent\n          \}$/);
});

test('embedded publish transaction is valid Bash', async () => {
  const result = spawnSync('bash', ['-n'], {
    input: await readPublishScript(),
    encoding: 'utf8',
  });

  assert.equal(result.status, 0, result.stderr);
});
