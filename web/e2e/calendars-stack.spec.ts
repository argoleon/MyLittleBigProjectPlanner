/**
 * Calendars on the production stack, nothing mocked (L07-US04): the admin
 * creates a site using the organization's French holidays, a resource on
 * that site, and finds 25 December 2026 shown as Christmas on the month
 * grid of the resource, reached with the keyboard.
 *
 * Runs with `just compose-e2e` only: deploy/tests/e2e.sh creates the admin
 * and passes E2E_ADMIN_EMAIL and E2E_ADMIN_SECRET.
 */
import { expect, test } from '@playwright/test';

import { accessibilityViolations } from './fixtures';

const adminEmail = process.env['E2E_ADMIN_EMAIL'];
const adminSecret = process.env['E2E_ADMIN_SECRET'];

test.skip(!adminEmail || !adminSecret, 'needs the compose stack (just compose-e2e)');

test('a resource of the Paris site does not work on Christmas', async ({ browser }) => {
  test.setTimeout(120_000);
  const stamp = Date.now();
  const site = `E2E Paris ${stamp}`;
  const name = `E2E Calendar ${stamp}`;

  const context = await browser.newContext();
  await context.addInitScript(() => localStorage.setItem('mlbpp.language', 'en'));
  const page = await context.newPage();
  await page.goto('/sign-in');
  await page.getByLabel('Email').fill(adminEmail!);
  await page.getByLabel('Password').fill(adminSecret!);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');

  // The site, on the Calendars tab.
  await page.goto('/workload/calendars');
  await expect(page.getByRole('row').filter({ hasText: 'Organization calendar' })).toBeVisible();
  await page.getByRole('button', { name: 'New site' }).click();
  let dialog = page.getByRole('dialog');
  await dialog.getByLabel('Name (English)').fill(site);
  await dialog.getByLabel('Name (Français)').fill(site);
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(page.getByRole('status')).toHaveText(`${site} was created.`);
  expect(await accessibilityViolations(page), 'calendars tab').toEqual([]);

  // A resource on that site.
  await page.goto('/workload/resources');
  await page.getByRole('button', { name: 'New resource' }).click();
  dialog = page.getByRole('dialog');
  await dialog.getByLabel('Name').fill(name);
  await dialog.getByRole('combobox', { name: 'Site' }).click();
  await page.getByRole('option', { name: site }).click();
  await dialog.getByRole('button', { name: 'Create' }).click();
  await page.getByRole('link', { name }).click();
  await expect(page.getByText(`Site ${site}`)).toBeVisible();

  // December 2026, month after month.
  const calendar = page.locator('app-month-calendar');
  const title = calendar.getByRole('heading', { level: 2 });
  await expect(title).not.toBeEmpty();
  for (let i = 0; i < 48 && (await title.textContent())?.trim() !== 'December 2026'; i++) {
    const current = new Date(`1 ${(await title.textContent())?.trim()}`);
    const forward = current < new Date(2026, 11, 1);
    await calendar.getByRole('button', { name: forward ? 'Next month' : 'Previous month' }).click();
  }
  await expect(title).toHaveText('December 2026');
  const christmas = calendar.locator('[data-day="2026-12-25"]');
  await expect(christmas).toHaveAttribute('data-look', 'holiday');
  await expect(christmas).toHaveAccessibleName(/^25 Christmas\s*, Friday, December 25, 2026, public holiday$/);

  // The keyboard moves within the grid.
  await calendar.locator('[data-day="2026-12-18"]').focus();
  await page.keyboard.press('ArrowDown');
  await expect(christmas).toBeFocused();
  expect(await accessibilityViolations(page), 'resource page with its calendar').toEqual([]);
  await context.close();
});
