// Checks that stylelint still rejects every hard-coded value of
// fixtures/hard-coded-values.scss (one per line, lines 4 to 9), so the theme
// rule of .stylelintrc.json cannot be weakened without notice.
//
// Usage: node scripts/check-style-rules.mjs (from web/; part of `just web-lint`).
import stylelint from 'stylelint';

const FIXTURE = 'scripts/fixtures/hard-coded-values.scss';
const EXPECTED_LINES = [4, 5, 6, 7, 8, 9];

const { results } = await stylelint.lint({ files: [FIXTURE], configFile: '.stylelintrc.json' });
const flagged = new Set(results.flatMap((r) => r.warnings.map((w) => w.line)));
const missed = EXPECTED_LINES.filter((line) => !flagged.has(line));
if (missed.length > 0) {
  console.error(`stylelint no longer rejects the hard-coded values of ${FIXTURE}, lines ${missed.join(', ')}.`);
  process.exit(1);
}
console.log(`Theme rule OK: the ${EXPECTED_LINES.length} hard-coded values of the fixture are rejected.`);
