// Fails if a translation key used in the code is missing in fr.json or
// en.json, or if the two files do not have the same keys.
//
// A key is any string literal of the sources (`.ts` and `.html` under
// projects/*/src) that starts with a top-level section of the translation
// files followed by a dot, e.g. 'nav.workload'. Keys built at runtime
// ('status.' + category) are covered by unit tests instead.
//
// Usage: node scripts/check-i18n.mjs (from web/; `just web-i18n`).
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';

const I18N_DIR = 'projects/ui/i18n';
const SOURCES = ['projects/app/src', 'projects/portal/src', 'projects/ui/src'];
const SKIP = [/\.spec\.ts$/, /\/generated\//, /\/testing\//];

const load = (lang) => JSON.parse(readFileSync(join(I18N_DIR, `${lang}.json`), 'utf8'));

function flatten(tree, prefix = '') {
  return Object.entries(tree).flatMap(([key, value]) =>
    typeof value === 'string' ? [prefix + key] : flatten(value, `${prefix}${key}.`),
  );
}

function files(dir) {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return files(path);
    return /\.(ts|html)$/.test(name) && !SKIP.some((re) => re.test(path)) ? [path] : [];
  });
}

const fr = load('fr');
const en = load('en');
const frKeys = new Set(flatten(fr));
const enKeys = new Set(flatten(en));
const problems = [];

for (const key of frKeys) if (!enKeys.has(key)) problems.push(`${key}: missing in en.json`);
for (const key of enKeys) if (!frKeys.has(key)) problems.push(`${key}: missing in fr.json`);

const sections = Object.keys(en).join('|');
const literal = new RegExp(`['"\`]((?:${sections})\\.[A-Za-z0-9_.]+)['"\`]`, 'g');
let used = 0;
for (const file of SOURCES.flatMap(files)) {
  const text = readFileSync(file, 'utf8');
  for (const match of text.matchAll(literal)) {
    const key = match[1];
    used++;
    // A key ending with a dot is a prefix completed at runtime.
    if (key.endsWith('.')) continue;
    for (const [lang, keys] of [['fr', frKeys], ['en', enKeys]]) {
      if (!keys.has(key)) problems.push(`${key}: used in ${relative('.', file)}, missing in ${lang}.json`);
    }
  }
}

if (problems.length > 0) {
  console.error(`Translation problems:\n${[...new Set(problems)].map((p) => `  - ${p}`).join('\n')}`);
  process.exit(1);
}
console.log(`Translations OK: ${enKeys.size} keys in fr.json and en.json, ${used} uses in the code.`);
