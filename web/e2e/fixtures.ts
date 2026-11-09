import AxeBuilder from '@axe-core/playwright';
import { Page, test as base, expect } from '@playwright/test';

export { expect };

/** The tenant the mocked API returns. */
export const TENANT = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a8b',
  slug: 'acme',
  name: 'Acme Industrie',
  defaultLocale: 'fr',
  timeZone: 'Europe/Paris',
};

/** Pages start with the API mocked and the given language and theme. */
export const test = base.extend<{ language: 'fr' | 'en'; theme: 'system' | 'light' | 'dark' }>({
  language: ['en', { option: true }],
  theme: ['system', { option: true }],
  page: async ({ page, language, theme }, use) => {
    await page.route('**/api/v1/tenant', (route) => route.fulfill({ json: TENANT }));
    await page.addInitScript(
      ([lang, mode]) => {
        // Only on the first load: later loads keep what the user chose.
        if (!sessionStorage.getItem('e2e.initialized')) {
          localStorage.setItem('mlbpp.language', lang);
          localStorage.setItem('mlbpp.theme', mode);
          sessionStorage.setItem('e2e.initialized', 'true');
        }
      },
      [language, theme] as const,
    );
    await use(page);
  },
});

/** WCAG 2.1 A and AA violations of the page, color contrast included. */
export async function accessibilityViolations(page: Page): Promise<string[]> {
  const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
  return results.violations.map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`);
}
