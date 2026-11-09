// Tests of check-licenses.mjs on fixtures/licenses/: what the license
// policy accepts and rejects. Run with `node --test` (`just web-licenses`).
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { test } from 'node:test';

const run = (...args) =>
  spawnSync(process.execPath, ['scripts/check-licenses.mjs', ...args], { encoding: 'utf8' });

const fixture = run(
  '--lock',
  'scripts/fixtures/licenses/package-lock.json',
  '--bundle',
  'scripts/fixtures/licenses/browser-stats.json',
);

test('the fixture fails the check', () => {
  assert.equal(fixture.status, 1);
});

test('GPL and AGPL packages are rejected, runtime or tool', () => {
  assert.match(fixture.stderr, /gpl-runtime@1\.0\.0: GPL-3\.0-only/);
  assert.match(fixture.stderr, /agpl-tool@1\.0\.0: AGPL-3\.0-or-later/);
});

test('a tools-only license is rejected once in a shipped bundle', () => {
  assert.match(fixture.stderr, /tool-license-in-bundle@1\.0\.0: MPL-2\.0 is only allowed for tools/);
});

test('a package without license is rejected', () => {
  assert.match(fixture.stderr, /no-license@1\.0\.0: no license declared/);
});

test('allowed packages pass, including tools and dual licenses with an allowed choice', () => {
  for (const name of ['allowed-runtime', 'allowed-tool', 'dual-licensed']) {
    assert.doesNotMatch(fixture.stderr, new RegExp(`${name}@`));
  }
});

test('missing bundle metafiles are reported', () => {
  const result = run('--lock', 'scripts/fixtures/licenses/package-lock.json', '--bundle', 'does-not-exist.json');
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Missing does-not-exist\.json/);
});
