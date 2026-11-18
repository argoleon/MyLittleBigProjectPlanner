// Vulnerabilities of the npm dependencies (CLAUDE.md, `just web-audit`).
//
// What ships (`npm audit --omit=dev`) may have no known vulnerability at
// all: the recipe checks that first, without exceptions. This script checks
// everything else, build and test tools included: no high or critical
// advisory, but those listed in audit-exceptions.json. An exception names
// the advisory and its package, why it is accepted and until when; past that
// date it fails again, to be reviewed. An exception nothing matches any more
// is reported, to be removed.
//
// Usage: node scripts/check-audit.mjs (from web/; runs `npm audit --json`).
// Tests pass other files:
//   node scripts/check-audit.mjs --report <audit.json> --exceptions <file> --today <YYYY-MM-DD>
// (see check-audit.test.mjs).
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { parseArgs } from 'node:util';

const BLOCKING = ['high', 'critical'];
const { values: args } = parseArgs({
  options: {
    report: { type: 'string' },
    exceptions: { type: 'string', default: 'audit-exceptions.json' },
    today: { type: 'string', default: new Date().toISOString().slice(0, 10) },
  },
});

function fail(message) {
  console.error(message);
  process.exit(1);
}

/** The report of `npm audit --json`: from `--report`, else by running it. */
function report() {
  let text;
  if (args.report) {
    text = readFileSync(args.report, 'utf8');
  } else {
    // npm audit exits with 1 when it finds anything: the report says what.
    const audit = spawnSync('npm', ['audit', '--json'], { encoding: 'utf8' });
    text = audit.stdout;
  }
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch {
    fail(`npm audit did not answer a report:\n${text.slice(0, 500)}`);
  }
  if (parsed.error || typeof parsed.vulnerabilities !== 'object') {
    fail(`npm audit failed: ${JSON.stringify(parsed.error ?? parsed).slice(0, 500)}`);
  }
  return parsed;
}

/** The advisories of the report (each once), with the package they are about. */
function advisories(audit) {
  const found = new Map();
  for (const vulnerability of Object.values(audit.vulnerabilities)) {
    for (const via of vulnerability.via) {
      // A string names another vulnerable package: its own entry says why.
      if (typeof via !== 'object') continue;
      const id = via.url?.split('/').pop() ?? String(via.source);
      found.set(`${id} ${via.name}`, { id, name: via.name, severity: via.severity, title: via.title, range: via.range });
    }
  }
  return [...found.values()];
}

const exceptions = JSON.parse(readFileSync(args.exceptions, 'utf8')).exceptions;
const used = new Set();
const problems = [];
const accepted = [];
for (const advisory of advisories(report())) {
  if (!BLOCKING.includes(advisory.severity)) continue;
  const label = `${advisory.name} ${advisory.range}: ${advisory.id} (${advisory.severity}) ${advisory.title}`;
  const exception = exceptions.find((e) => e.advisory === advisory.id && e.package === advisory.name);
  if (!exception) {
    problems.push(label);
  } else if (exception.until < args.today) {
    used.add(exception);
    problems.push(`${label}\n    its exception ended on ${exception.until}: review it (audit-exceptions.json)`);
  } else {
    used.add(exception);
    accepted.push(`${label}\n    accepted until ${exception.until}: ${exception.reason}`);
  }
}

for (const line of accepted) console.log(`Accepted: ${line}`);
for (const exception of exceptions.filter((e) => !used.has(e))) {
  console.warn(`Unused exception, to remove from audit-exceptions.json: ${exception.advisory} ${exception.package}`);
}
if (problems.length > 0) {
  fail(`High or critical vulnerabilities in the npm dependencies:\n${problems.map((p) => `  ${p}`).join('\n')}`);
}
console.log('No high or critical vulnerability without an exception.');
