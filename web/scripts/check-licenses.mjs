// License check of the npm dependencies (CLAUDE.md, "Licensing").
//
// Every package of package-lock.json must be under an allowed license.
// Build and test tools that never end up in the shipped bundles may also be
// under the TOOLS_ONLY licenses (decided on 2026-11-09). That condition is
// checked on the bundles themselves: the esbuild metafiles written by
// `ng build --stats-json` list every file each bundle contains.
//
// Usage: node scripts/check-licenses.mjs (from web/; `just web-licenses`,
// after `just web-build`).
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

const ALLOWED = ['MIT', 'Apache-2.0', 'BSD-2-Clause', 'BSD-3-Clause', 'ISC', '0BSD', 'OFL-1.1'];
const TOOLS_ONLY = ['Python-2.0', 'MPL-2.0', 'CC-BY-4.0', 'BlueOak-1.0.0', 'CC0-1.0', 'MIT-0'];
const BUNDLES = ['dist/app/browser-stats.json', 'dist/portal/browser-stats.json'];

/** Whether an SPDX expression is satisfied by the `accepted` licenses. */
function satisfies(expression, accepted) {
  const text = expression.replace(/[()]/g, ' ').trim();
  // "A OR B": one is enough; "A AND B": all are needed.
  return text
    .split(/\s+OR\s+/)
    .some((alternative) => alternative.split(/\s+AND\s+/).every((license) => accepted.includes(license.trim())));
}

/** The license of a lock entry; old packages declare `licenses: [{type}]` instead. */
function licenseOf(path, entry) {
  if (typeof entry.license === 'string') return entry.license;
  const manifest = join(path, 'package.json');
  if (!existsSync(manifest)) return '';
  const { license, licenses } = JSON.parse(readFileSync(manifest, 'utf8'));
  if (typeof license === 'string') return license;
  if (Array.isArray(licenses)) return licenses.map((l) => l.type).join(' OR ');
  return '';
}

const missingBundles = BUNDLES.filter((file) => !existsSync(file));
if (missingBundles.length > 0) {
  console.error(`Missing ${missingBundles.join(', ')}: run \`just web-build\` first.`);
  process.exit(1);
}
const bundled = BUNDLES.flatMap((file) => Object.keys(JSON.parse(readFileSync(file, 'utf8')).inputs));
const inBundle = (path) => bundled.some((input) => input.startsWith(`${path}/`));

const lock = JSON.parse(readFileSync('package-lock.json', 'utf8'));
const problems = [];
let count = 0;
let toolsOnly = 0;
for (const [path, entry] of Object.entries(lock.packages)) {
  if (path === '' || entry.link) continue;
  count++;
  const name = `${path.replace(/^.*node_modules\//, '')}@${entry.version}`;
  const license = licenseOf(path, entry);
  if (!license) {
    problems.push(`${name}: no license declared`);
  } else if (satisfies(license, ALLOWED)) {
    continue;
  } else if (!satisfies(license, [...ALLOWED, ...TOOLS_ONLY])) {
    problems.push(`${name}: ${license}`);
  } else if (inBundle(path)) {
    problems.push(`${name}: ${license} is only allowed for tools, but it is in a shipped bundle`);
  } else {
    toolsOnly++;
  }
}

if (problems.length > 0) {
  console.error(`Dependencies under a license that is not allowed:\n${problems.map((p) => `  - ${p}`).join('\n')}`);
  process.exit(1);
}
console.log(`Licenses OK: ${count} packages, ${toolsOnly} of them tools under a tools-only license, none in the bundles.`);
