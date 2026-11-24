/**
 * Absences on the production stack, nothing mocked (L07-US05): a member
 * linked to a resource reaches it from My account, declares a half day of
 * training there, sees it on the card with its comment, then deletes it.
 *
 * Runs with `just compose-e2e` only: deploy/tests/e2e.sh creates the admin
 * and passes E2E_ADMIN_EMAIL and E2E_ADMIN_SECRET. The admin plays the
 * member: linked to a resource on the first run, kept for the next ones.
 */
import { expect, test } from '@playwright/test';

import { accessibilityViolations } from './fixtures';

const adminEmail = process.env['E2E_ADMIN_EMAIL'];
const adminSecret = process.env['E2E_ADMIN_SECRET'];

test.skip(!adminEmail || !adminSecret, 'needs the compose stack (just compose-e2e)');

test('a member declares their own half day off from My account', async ({ browser }) => {
  test.setTimeout(120_000);
  const comment = `E2E training ${Date.now()}`;

  const context = await browser.newContext();
  await context.addInitScript(() => localStorage.setItem('mlbpp.language', 'en'));
  const page = await context.newPage();
  await page.goto('/sign-in');
  await page.getByLabel('Email').fill(adminEmail!);
  await page.getByLabel('Password').fill(adminSecret!);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');

  // Linked to a resource, once.
  const mine = page.waitForResponse((r) => r.url().endsWith('/api/v1/resources/mine'));
  await page.goto('/account');
  const linked = (await mine).ok();
  const myAbsences = page.getByRole('link', { name: 'My absences' });
  if (!linked) {
    await page.goto('/workload/resources');
    await page.getByRole('button', { name: 'New resource' }).click();
    const dialog = page.getByRole('dialog');
    await dialog.getByRole('combobox', { name: 'Linked account' }).fill(adminEmail!);
    await page.getByRole('option', { name: new RegExp(adminEmail!.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')) }).click();
    await dialog.getByRole('button', { name: 'Create' }).click();
    await expect(page.getByRole('status')).toContainText('was created.');
    await page.goto('/account');
  }
  await myAbsences.click();
  await expect(page).toHaveURL(/\/workload\/resources\/[0-9a-f-]+$/);

  // A half day of training next month.
  const next = new Date();
  next.setMonth(next.getMonth() + 1, 15);
  const day = `${next.getMonth() + 1}/${next.getDate()}/${next.getFullYear()}`;
  await page.getByRole('button', { name: 'Declare an absence' }).first().click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('combobox', { name: 'Kind' }).click();
  await page.getByRole('option', { name: 'Training' }).click();
  await dialog.getByLabel('First day').fill(day);
  await dialog.getByRole('switch', { name: /Starts at noon/ }).click();
  await dialog.getByLabel('Comment').fill(comment);
  expect(await accessibilityViolations(page), 'absence dialog').toEqual([]);
  await dialog.getByRole('button', { name: 'Declare' }).click();
  const card = page.locator('app-absences-card');
  await expect(card.getByRole('status')).toHaveText('The absence was declared.');
  const row = card.getByRole('listitem').filter({ hasText: comment });
  await expect(row).toContainText('Training');
  await expect(row).toContainText('afternoon');
  expect(await accessibilityViolations(page), 'resource page with absences').toEqual([]);

  // Gone again, so that the next run finds the day free.
  await row.getByRole('button', { name: /^Delete the absence/ }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Delete' }).click();
  await expect(card.getByRole('status')).toHaveText('The absence was deleted.');
  await expect(card.getByText(comment)).toHaveCount(0);
  await context.close();
});
