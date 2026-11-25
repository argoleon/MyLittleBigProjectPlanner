/**
 * Teams on the production stack, nothing mocked (L07-US02): the tenant
 * admin creates a team, adds a resource to it from the team page, finds it
 * with the team filter of the resource list, then removes it.
 *
 * Runs with `just compose-e2e` only: deploy/tests/e2e.sh creates the admin
 * and passes E2E_ADMIN_EMAIL and E2E_ADMIN_SECRET.
 */
import { expect, test } from '@playwright/test';

import { accessibilityViolations } from './fixtures';

const adminEmail = process.env['E2E_ADMIN_EMAIL'];
const adminSecret = process.env['E2E_ADMIN_SECRET'];

test.skip(!adminEmail || !adminSecret, 'needs the compose stack (just compose-e2e)');

test('create a team, add a resource to it, filter by it, remove it', async ({ browser }) => {
  test.setTimeout(90_000);
  const stamp = Date.now();
  const role = `E2E Tester ${stamp}`;
  const team = `E2E Team ${stamp}`;

  const context = await browser.newContext();
  await context.addInitScript(() => localStorage.setItem('mlbpp.language', 'en'));
  const page = await context.newPage();
  await page.goto('/sign-in');
  await page.getByLabel('Email').fill(adminEmail!);
  await page.getByLabel('Password').fill(adminSecret!);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');

  // A resource to put in the team.
  await page.goto('/workload/resources');
  await page.getByRole('button', { name: 'New resource' }).click();
  let dialog = page.getByRole('dialog');
  await dialog.getByText('Generic resource', { exact: true }).click();
  await dialog.getByLabel('Role').fill(role);
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(page.getByRole('status')).toHaveText(`${role} was created.`);

  // The team.
  await page.getByRole('tab', { name: 'Teams' }).click();
  await expect(page).toHaveURL(/\/workload\/teams$/);
  await page.getByRole('button', { name: 'New team' }).click();
  dialog = page.getByRole('dialog');
  await dialog.getByLabel('Name (English)').fill(team);
  await dialog.getByLabel('Name (Français)').fill(team);
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(page.getByRole('status')).toHaveText(`${team} was created.`);
  expect(await accessibilityViolations(page), 'teams tab').toEqual([]);

  // Its page: the resource added.
  await page.getByRole('link', { name: team }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(team);
  await page.getByRole('button', { name: 'Add a resource' }).click();
  dialog = page.getByRole('dialog');
  await dialog.getByRole('combobox', { name: 'Resource' }).fill(String(stamp));
  await page.getByRole('option', { name: new RegExp(role) }).click();
  await dialog.getByRole('button', { name: 'Add' }).click();
  await expect(page.getByRole('status')).toHaveText(`${role} was added to the team.`);
  await expect(page.getByRole('heading', { level: 2 })).toHaveText('Resources (1)');
  await expect(page.getByRole('row').filter({ hasText: role })).toBeVisible();
  expect(await accessibilityViolations(page), 'team page').toEqual([]);

  // The resource list filtered by the team.
  await page.goto('/workload/resources');
  await page.getByRole('combobox', { name: 'Team' }).click();
  await page.getByRole('option', { name: team }).click();
  await expect(page.getByRole('row').filter({ hasText: role })).toBeVisible();
  await expect(page.locator('tr[mat-row]')).toHaveCount(1);

  // Out of the team again.
  await page.getByRole('link', { name: role }).click();
  await page.getByRole('link', { name: team }).click();
  await page.getByRole('button', { name: `Remove ${role} from the team` }).click();
  await expect(page.getByRole('status')).toHaveText(`${role} was removed from the team.`);
  await expect(page.getByText('No resource in this team')).toBeVisible();
  await context.close();
});
