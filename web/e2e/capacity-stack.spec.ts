/**
 * Capacity on the production stack, nothing mocked (L07-US03): a new
 * resource shows the organization default; the admin adds a part-time
 * period with a Friday of three and a half hours, which the table lists
 * with its weekly total. The workload settings tab of the administration
 * shows the default capacity.
 *
 * Runs with `just compose-e2e` only: deploy/tests/e2e.sh creates the admin
 * and passes E2E_ADMIN_EMAIL and E2E_ADMIN_SECRET.
 */
import { expect, test } from '@playwright/test';

import { accessibilityViolations } from './fixtures';

const adminEmail = process.env['E2E_ADMIN_EMAIL'];
const adminSecret = process.env['E2E_ADMIN_SECRET'];

test.skip(!adminEmail || !adminSecret, 'needs the compose stack (just compose-e2e)');

test('add a capacity period to a resource and read it back', async ({ browser }) => {
  test.setTimeout(90_000);
  const name = `E2E Capacity ${Date.now()}`;

  const context = await browser.newContext();
  await context.addInitScript(() => localStorage.setItem('mlbpp.language', 'en'));
  const page = await context.newPage();
  await page.goto('/sign-in');
  await page.getByLabel('Email').fill(adminEmail!);
  await page.getByLabel('Password').fill(adminSecret!);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');

  // The organization's workload settings.
  await page.goto('/admin/workload');
  await expect(page.getByRole('heading', { name: 'Default capacity' })).toBeVisible();
  expect(await accessibilityViolations(page), 'workload settings').toEqual([]);

  // A resource, then its page.
  await page.goto('/workload/resources');
  await page.getByRole('button', { name: 'New resource' }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByLabel('Name').fill(name);
  await dialog.getByRole('button', { name: 'Create' }).click();
  await page.getByRole('link', { name }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(name);
  const card = page.locator('app-capacity-card');
  await expect(card.getByRole('row').filter({ hasText: 'Organization default' })).toBeVisible();

  // A part-time period.
  await card.getByRole('button', { name: 'New period' }).click();
  const period = page.getByRole('dialog');
  await period.getByLabel('From').fill('3/1/2026');
  await period.getByLabel('Wednesday').fill('0');
  await period.getByLabel('Friday').fill('3,5');
  await period.getByLabel('Label').fill('Part time');
  await period.getByRole('button', { name: 'Add' }).click();
  await expect(card.getByRole('status')).toHaveText('The period was added.');
  const row = card.getByRole('row').filter({ hasText: 'Mar 1, 2026' });
  await expect(row).toContainText('Part time');
  await expect(row).toContainText('3 h 30 min');
  await expect(row).toContainText('24 h 30 min');
  await expect(card).toContainText('Before Mar 1, 2026: the organization default');
  expect(await accessibilityViolations(page), 'resource page with capacity').toEqual([]);
  await context.close();
});
