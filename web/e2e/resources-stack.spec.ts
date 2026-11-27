/**
 * Resources on the production stack, nothing mocked (L07-US01): the tenant
 * admin opens Workload, creates a person without account and a generic
 * resource, opens the generic one and deactivates it; it then only shows
 * among inactive resources.
 *
 * Runs with `just compose-e2e` only: deploy/tests/e2e.sh creates the admin
 * and passes E2E_ADMIN_EMAIL and E2E_ADMIN_SECRET.
 */
import { expect, test } from '@playwright/test';

import { accessibilityViolations } from './fixtures';

const adminEmail = process.env['E2E_ADMIN_EMAIL'];
const adminSecret = process.env['E2E_ADMIN_SECRET'];

test.skip(!adminEmail || !adminSecret, 'needs the compose stack (just compose-e2e)');

test('create a person and a generic resource, then deactivate one', async ({ browser }) => {
  test.setTimeout(90_000);
  const stamp = Date.now();
  const contractor = `E2E Contractor ${stamp}`;
  const role = `E2E Rust developer ${stamp}`;

  const context = await browser.newContext();
  await context.addInitScript(() => localStorage.setItem('mlbpp.language', 'en'));
  const page = await context.newPage();
  await page.goto('/sign-in');
  await page.getByLabel('Email').fill(adminEmail!);
  await page.getByLabel('Password').fill(adminSecret!);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');

  // Workload opens on its heatmap; its resources are the next tab.
  await page.getByRole('link', { name: 'Workload' }).click();
  await expect(page).toHaveURL(/\/workload\/heatmap$/);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Workload');
  await page.getByRole('tab', { name: 'Resources' }).click();
  await expect(page).toHaveURL(/\/workload\/resources$/);
  await expect(page.getByRole('tab', { name: 'Resources' })).toHaveAttribute('aria-selected', 'true');

  // A person without account.
  await page.getByRole('button', { name: 'New resource' }).click();
  let dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('heading')).toHaveText('New resource');
  expect(await accessibilityViolations(page), 'resource dialog').toEqual([]);
  await dialog.getByLabel('Name').fill(contractor);
  await dialog.getByLabel('Job title').fill('Subcontractor');
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(page.getByRole('status')).toHaveText(`${contractor} was created.`);

  // A generic resource.
  await page.getByRole('button', { name: 'New resource' }).click();
  dialog = page.getByRole('dialog');
  await dialog.getByText('Generic resource', { exact: true }).click();
  await dialog.getByLabel('Role').fill(role);
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(page.getByRole('status')).toHaveText(`${role} was created.`);
  await page.getByLabel('Search').fill(String(stamp));
  await expect(page.getByRole('row').filter({ hasText: contractor })).toContainText('Resource type: Person');
  await expect(page.getByRole('row').filter({ hasText: role })).toContainText('Resource type: Generic resource');
  expect(await accessibilityViolations(page), 'resource list').toEqual([]);

  // Its page, then deactivated.
  await page.getByRole('link', { name: role }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(role);
  expect(await accessibilityViolations(page), 'resource page').toEqual([]);
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  dialog = page.getByRole('dialog');
  await dialog.getByRole('switch', { name: 'Active' }).click();
  await dialog.getByRole('button', { name: 'Save' }).click();
  await expect(dialog).toBeHidden();
  await expect(page.locator('ui-resource-state-chip')).toHaveText(/Inactive/);

  // Only among inactive resources now.
  await page.getByRole('link', { name: 'Resources' }).click();
  await page.getByLabel('Search').fill(String(stamp));
  await expect(page.getByRole('row').filter({ hasText: contractor })).toBeVisible();
  await expect(page.getByRole('row').filter({ hasText: role })).toHaveCount(0);
  await page.getByRole('option', { name: 'Inactive', exact: true }).click();
  await expect(page.getByRole('row').filter({ hasText: role })).toContainText('Status: Inactive');
  await context.close();
});
