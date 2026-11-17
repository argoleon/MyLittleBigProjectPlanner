// Tests of check-audit.mjs on fixtures/audit/: which advisories fail the
// audit, and what an exception lets through. Run with `node --test`
// (`just web-audit`).
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { test } from 'node:test';

const fixtures = 'scripts/fixtures/audit';
const run = (report, exceptions, today = '2026-11-17') =>
  spawnSync(
    process.execPath,
    [
      'scripts/check-audit.mjs',
      '--report',
      `${fixtures}/${report}`,
      '--exceptions',
      `${fixtures}/${exceptions}`,
      '--today',
      today,
    ],
    { encoding: 'utf8' },
  );

test('a high advisory without exception fails, named with its package', () => {
  const result = run('high.json', 'none.json');
  assert.equal(result.status, 1);
  assert.match(result.stderr, /picomatch <=4\.0\.7: GHSA-fc5q-39xc-vjpf \(high\)/);
});

test('a high advisory with an exception passes until its date, with the reason shown', () => {
  const result = run('high.json', 'exceptions.json');
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /Accepted: picomatch .* accepted until 2027-01-15: tools only/s);
  assert.equal(run('high.json', 'exceptions.json', '2027-01-15').status, 0, 'the last day included');
});

test('past its date, the exception fails again', () => {
  const result = run('high.json', 'exceptions.json', '2027-01-16');
  assert.equal(result.status, 1);
  assert.match(result.stderr, /its exception ended on 2027-01-15: review it/);
});

test('an exception covers its package only', () => {
  const result = run('high.json', 'exceptions-other-package.json');
  assert.equal(result.status, 1);
  assert.match(result.stderr, /picomatch <=4\.0\.7: GHSA-fc5q-39xc-vjpf/);
});

test('moderate and lower advisories of the tools do not fail', () => {
  assert.doesNotMatch(run('high.json', 'none.json').stderr, /yaml/);
});

test('a critical advisory fails', () => {
  const result = run('critical.json', 'none.json');
  assert.equal(result.status, 1);
  assert.match(result.stderr, /evil-tool \*: GHSA-cccc-cccc-cccc \(critical\) Remote code execution/);
});

test('an exception nothing matches is reported, to be removed', () => {
  const result = run('high.json', 'exceptions.json');
  assert.match(result.stderr, /Unused exception, to remove from audit-exceptions\.json: GHSA-gone-gone-gone fixed-since/);
});

test('a failed audit (no report) fails', () => {
  const result = run('error.json', 'none.json');
  assert.equal(result.status, 1);
  assert.match(result.stderr, /npm audit failed: .*ENOTFOUND/);
});

test('the exceptions of the repository are well formed', async () => {
  const { readFile } = await import('node:fs/promises');
  const { exceptions } = JSON.parse(await readFile('audit-exceptions.json', 'utf8'));
  for (const exception of exceptions) {
    assert.match(exception.advisory, /^GHSA-[a-z0-9]{4}-[a-z0-9]{4}-[a-z0-9]{4}$/, JSON.stringify(exception));
    assert.match(exception.until, /^\d{4}-\d{2}-\d{2}$/);
    assert.match(exception.decided, /^\d{4}-\d{2}-\d{2}$/);
    assert.ok(exception.package && exception.reason, JSON.stringify(exception));
  }
});
