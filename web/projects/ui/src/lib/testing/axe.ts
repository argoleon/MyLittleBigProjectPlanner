import axe from 'axe-core';

/**
 * axe-core violations (WCAG 2.1 A and AA rules) inside `element`. jsdom does
 * not render styles, so color contrast is checked by the end-to-end tests.
 */
export async function axeViolations(element: Element): Promise<string[]> {
  const results = await axe.run(element, {
    runOnly: { type: 'tag', values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'] },
    rules: { 'color-contrast': { enabled: false } },
  });
  return results.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`);
}
