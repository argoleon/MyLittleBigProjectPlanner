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

/** The signed-in member's tenant, in `GET /api/v1/me/memberships`. */
export const MEMBERSHIP = {
  tenantId: TENANT.id,
  slug: TENANT.slug,
  name: TENANT.name,
  url: 'https://acme.planner.example',
  accountType: 'internal',
  tenantRole: 'member',
  current: true,
};

/** The member signed in by default. */
export const SESSION = {
  account: {
    id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a8c',
    email: 'lucas.bernard@acme.example',
    displayName: 'Lucas Bernard',
    locale: null,
    tenantRole: 'member',
  },
  stage: 'complete',
  csrfToken: 'csrf-token',
  expiresAt: '2026-11-17T09:00:00Z',
};

/** The signed-in account, in `GET /api/v1/me`: no preferences of its own. */
export const ME = {
  id: SESSION.account.id,
  email: SESSION.account.email,
  displayName: SESSION.account.displayName,
  locale: null,
  timeZone: null,
  tenantId: TENANT.id,
  tenantRole: 'member',
  accountType: 'internal',
};

/** A Problem Details response. */
export function problem(status: number, code: string, errors: object[] = []) {
  return {
    status,
    contentType: 'application/problem+json',
    body: JSON.stringify({ type: `urn:mlbpp:problem:${code}`, title: code, status, code, errors }),
  };
}

/**
 * Pages start with the API mocked (signed in unless `signedIn` is false)
 * and the given language and theme.
 */
export const test = base.extend<{ language: 'fr' | 'en'; theme: 'system' | 'light' | 'dark'; signedIn: boolean }>({
  language: ['en', { option: true }],
  theme: ['system', { option: true }],
  signedIn: [true, { option: true }],
  page: async ({ page, language, theme, signedIn }, use) => {
    await page.route('**/api/v1/tenant', (route) => route.fulfill({ json: TENANT }));
    await page.route('**/api/v1/me/memberships', (route) => route.fulfill({ json: [MEMBERSHIP] }));
    await page.route('**/api/v1/me', (route) => route.fulfill({ json: ME }));
    // Languages picked in the menu are saved for the account.
    await page.route('**/api/v1/me/preferences', (route) => route.fulfill({ json: route.request().postDataJSON() }));
    await page.route('**/api/v1/auth/session', (route) =>
      signedIn ? route.fulfill({ json: SESSION }) : route.fulfill(problem(401, 'unauthenticated')),
    );
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
