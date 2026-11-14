/**
 * Tickets on the production stack, nothing mocked (L05-US01): the tenant
 * admin creates a project, then a story from the project's page with a
 * rich description, lands on its page with the project's first key, finds
 * it in "My tickets", and adds a sub-task under it.
 *
 * Runs with `just compose-e2e` only: deploy/tests/e2e.sh creates the admin
 * and passes E2E_ADMIN_EMAIL and E2E_ADMIN_SECRET.
 */
import { type Page, expect, test } from '@playwright/test';

import { accessibilityViolations } from './fixtures';

const adminEmail = process.env['E2E_ADMIN_EMAIL'];
const adminSecret = process.env['E2E_ADMIN_SECRET'];

test.skip(!adminEmail || !adminSecret, 'needs the compose stack (just compose-e2e)');

async function signIn(page: Page, email: string, secret: string) {
  await page.context().addInitScript(() => localStorage.setItem('mlbpp.language', 'en'));
  await page.goto('/sign-in');
  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password').fill(secret);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');
}

/** Creates a project at the root of the hierarchy; the URL of its page. */
async function project(page: Page, name: string, key: string): Promise<string> {
  await page.goto('/portfolios');
  await page.getByRole('button', { name: 'New node' }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('combobox', { name: 'Type' }).click();
  await page.getByRole('option', { name: 'Project', exact: true }).click();
  await dialog.getByLabel('Name').fill(name);
  await dialog.getByLabel('Key').fill(key);
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(`${name} · Settings`);
  return page.url().replace(/\/general$/, '');
}

test('a story is created from its project with a rich description, then a sub-task under it', async ({ page }) => {
  test.setTimeout(120_000);
  const run = Date.now().toString(36).toUpperCase().slice(-5);
  const name = `Identity ${run}`;
  const code = `ID${run}`.slice(0, 10);
  await signIn(page, adminEmail!, adminSecret!);
  const url = await project(page, name, code);

  // From the project's page.
  await page.goto(url);
  await page.getByRole('button', { name: 'New ticket' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('heading', { name: 'New ticket' })).toBeVisible();
  await dialog.getByRole('combobox', { name: 'Type' }).click();
  await page.getByRole('option', { name: 'Story', exact: true }).click();
  // Nothing typed: the summary is required.
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(dialog.getByText('This field is required.').first()).toBeVisible();
  await dialog.getByLabel('Summary').fill('Migrate the authentication to OIDC');
  const description = dialog.getByRole('textbox', { name: 'Description' });
  await description.click();
  await page.keyboard.type('Replace the ');
  await page.keyboard.press('ControlOrMeta+b');
  await page.keyboard.type('local');
  await page.keyboard.press('ControlOrMeta+b');
  await page.keyboard.type(' accounts.');
  await dialog.getByRole('combobox', { name: 'Priority' }).click();
  await page.getByRole('option', { name: 'High', exact: true }).click();
  await dialog.getByRole('button', { name: 'Create' }).click();

  // Its page, with the project's first key.
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Migrate the authentication to OIDC');
  await expect(page).toHaveURL(new RegExp(`/issues/${code}-1$`));
  await expect(page.locator('.identity')).toContainText(`Story ${code}-1`);
  const shown = page.locator('ui-rich-text');
  await expect(shown).toHaveText('Replace the local accounts.');
  await expect(shown.locator('strong')).toHaveText('local');
  await expect(page.locator('.detail', { hasText: 'Priority' })).toContainText('High');
  await expect(page.locator('.detail', { hasText: 'Reporter' })).not.toContainText('—');
  expect(await accessibilityViolations(page)).toEqual([]);

  // In "My tickets".
  await page.getByRole('link', { name: 'My tickets' }).click();
  await expect(page.getByRole('link', { name: `${code}-1` })).toBeVisible();
  await page.getByRole('link', { name: `${code}-1` }).click();

  // A sub-task under it: the only type a story holds.
  await page.getByRole('button', { name: 'Add a sub-task' }).click();
  await expect(dialog.getByRole('heading', { name: `New sub-task of ${code}-1` })).toBeVisible();
  await expect(dialog.getByRole('combobox', { name: 'Type' })).toContainText('Sub-task');
  await dialog.getByLabel('Summary').fill('Configure the OIDC client');
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(page).toHaveURL(new RegExp(`/issues/${code}-2$`));
  await expect(page.getByRole('navigation', { name: 'Where the ticket belongs' })).toContainText(
    `${code}-1 Migrate the authentication to OIDC`,
  );
});
