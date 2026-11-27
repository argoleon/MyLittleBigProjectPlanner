/**
 * The availability search with the API mocked (L08-US10): the tab of the
 * Workload screen, a search for free hours, the results most free first
 * with their link to the heatmap; axe-core in light and dark modes, in
 * French and English.
 */
import type { Page } from '@playwright/test';

import { accessibilityViolations, expect, test } from './fixtures';

const FOUND = {
  from: '2026-11-23',
  to: '2026-11-29',
  includeProvisional: true,
  items: [
    {
      resource: { id: 'r3', kind: 'generic', displayName: 'Rust developer', linked: false },
      freeMinutes: 1560,
      freeDays: 3,
      capacityMinutes: 1680,
      loadMinutes: 120,
      workingDays: 4,
    },
    {
      resource: { id: 'r2', kind: 'person', displayName: 'Nathan Roux', jobTitle: 'Sysadmin', linked: true },
      freeMinutes: 1260,
      freeDays: 3,
      capacityMinutes: 2100,
      loadMinutes: 840,
      workingDays: 5,
    },
  ],
};

/** Mocks the teams and the search; the searches asked for. */
async function mockAvailability(page: Page): Promise<URL[]> {
  const asked: URL[] = [];
  await page.route('**/api/v1/teams', (route) =>
    route.fulfill({ json: { items: [{ id: 't1', name: 'Platform', nameTranslations: { en: 'Platform', fr: 'Plateforme' }, memberCount: 3, version: 1 }], canManage: false } }),
  );
  await page.route('**/api/v1/workload/availability?*', (route) => {
    asked.push(new URL(route.request().url()));
    return route.fulfill({ json: FOUND });
  });
  return asked;
}

test('the availability tab searches free hours in a team and lists the resources most free first', async ({ page }) => {
  const asked = await mockAvailability(page);
  await page.goto('/workload');
  await page.getByRole('tab', { name: 'Availability' }).click();
  await expect(page).toHaveURL(/\/workload\/availability$/);
  await expect(page.locator('table.found tbody tr')).toHaveCount(2);
  expect(asked[0]?.searchParams.get('minDays')).toBe('3');

  await page.locator('mat-form-field', { has: page.getByRole('combobox', { name: 'Team' }) }).click();
  await page.getByRole('option', { name: 'Platform' }).click();
  await page.locator('mat-button-toggle', { hasText: 'Free hours' }).locator('button').click();
  await page.getByLabel('At least').fill('10');
  await page.getByRole('button', { name: 'Search' }).click();
  await expect.poll(() => asked.length).toBe(2);
  expect(asked[1].searchParams.get('minHours')).toBe('10');
  expect(asked[1].searchParams.get('teamId')).toBe('t1');

  const rows = page.locator('table.found tbody tr');
  await expect(rows.first().locator('td').first()).toHaveText('26 h');
  await expect(rows.first().getByRole('link', { name: 'See in the heatmap: Rust developer' })).toHaveAttribute(
    'href',
    '/workload/heatmap?scale=day&date=2026-11-23&resourceId=r3&teamId=t1',
  );
});

for (const theme of ['light', 'dark'] as const) {
  for (const language of ['fr', 'en'] as const) {
    test.describe(`accessibility of the availability search, ${theme} mode, ${language}`, () => {
      test.use({ theme, language });

      test('the search and its results have no axe-core violation', async ({ page }) => {
        await mockAvailability(page);
        await page.goto('/workload/availability');
        await expect(page.locator('table.found tbody tr')).toHaveCount(2);
        expect(await accessibilityViolations(page)).toEqual([]);
      });
    });
  }
}
