/**
 * Several parents and the cycle check on the production stack, nothing
 * mocked (L03-US02): the tenant admin builds a portfolio, two programs, a
 * project and its sub-project, attaches the project to both programs with
 * weightings, then sees that its sub-project cannot become its parent.
 *
 * Runs with `just compose-e2e` only: deploy/tests/e2e.sh creates the admin
 * and passes E2E_ADMIN_EMAIL and E2E_ADMIN_SECRET.
 */
import { type Page, expect, test } from '@playwright/test';

import { accessibilityViolations } from './fixtures';

const adminEmail = process.env['E2E_ADMIN_EMAIL'];
const adminSecret = process.env['E2E_ADMIN_SECRET'];

test.skip(!adminEmail || !adminSecret, 'needs the compose stack (just compose-e2e)');

/** Creates a node from the open dialog, then waits for its settings. */
async function createIn(page: Page, type: string, name: string, key?: string) {
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('combobox', { name: 'Type' }).click();
  await page.getByRole('option', { name: type, exact: true }).click();
  await dialog.getByLabel('Name').fill(name);
  if (key) await dialog.getByLabel('Key').fill(key);
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(`${name} · Settings`);
}

async function addChild(page: Page, type: string, name: string, key?: string) {
  await page.getByRole('button', { name: 'Add a child' }).click();
  await createIn(page, type, name, key);
}

test('a project under two weighted programs, never under its own sub-project', async ({ browser }) => {
  test.setTimeout(120_000);
  // Unique per run: the stack keeps its data.
  const run = Date.now().toString(36).toUpperCase().slice(-6);
  const [digital, modernization, compliance] = [`Digital ${run}`, `Modernization ${run}`, `Compliance ${run}`];
  const [platform, permissions] = [`Platform ${run}`, `Permissions ${run}`];

  const context = await browser.newContext();
  await context.addInitScript(() => localStorage.setItem('mlbpp.language', 'en'));
  const page = await context.newPage();
  await page.goto('/sign-in');
  await page.getByLabel('Email').fill(adminEmail!);
  await page.getByLabel('Password').fill(adminSecret!);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');

  // A portfolio, a program, a project and its sub-project, each under the last.
  await page.goto('/portfolios');
  await page.getByRole('button', { name: 'New node' }).click();
  await createIn(page, 'Portfolio', digital);
  await addChild(page, 'Program', modernization);
  await addChild(page, 'Project', platform, `P${run}`);
  const platformUrl = page.url();
  await addChild(page, 'Project', permissions, `S${run}`);
  const breadcrumb = page.getByRole('navigation', { name: 'Breadcrumb' });
  // Back up to the portfolio, for the second program.
  await breadcrumb.getByRole('link', { name: platform }).click();
  await breadcrumb.getByRole('link', { name: modernization }).click();
  await breadcrumb.getByRole('link', { name: digital }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(`${digital} · Settings`);
  await addChild(page, 'Program', compliance);

  // The project: attached under the second program too.
  await page.goto(platformUrl);
  await page.getByRole('tab', { name: 'Attachments' }).click();
  const parents = page.getByRole('region', { name: 'Parents' });
  await expect(parents.getByRole('listitem')).toHaveCount(1);
  await page.getByRole('button', { name: 'Add a parent' }).click();
  const search = page.getByRole('region', { name: 'Add a parent' });
  await search.getByLabel('Search a node').fill(compliance);
  await search.getByRole('button', { name: new RegExp(`^${compliance} Program`) }).click();
  await expect(page.getByRole('status')).toHaveText(`Attached under ${compliance}.`);
  await expect(parents.getByRole('listitem')).toHaveCount(2);

  // Weightings: 40 % to Compliance, Modernization keeps 100 %.
  const weight = parents.getByRole('textbox', { name: `Weighting under ${compliance}` });
  await weight.fill('40');
  await weight.press('Enter');
  await expect(page.getByRole('status')).toHaveText(`Weighting under ${compliance} saved.`);
  await page.reload();
  await expect(parents.getByRole('textbox', { name: `Weighting under ${compliance}` })).toHaveValue('40');
  await expect(parents.getByRole('textbox', { name: `Weighting under ${modernization}` })).toHaveValue('100');
  await expect(page.getByRole('navigation', { name: 'Breadcrumb' })).toContainText(compliance);

  // Its sub-project cannot become its parent.
  await page.getByRole('button', { name: 'Add a parent' }).click();
  await search.getByLabel('Search a node').fill(permissions);
  const refused = search.getByRole('listitem').filter({ hasText: permissions });
  await expect(refused).toContainText('Impossible: it is a descendant of this node, the link would create a cycle');
  await expect(refused.getByRole('button')).toHaveCount(0);
  expect(await accessibilityViolations(page), 'attachments tab').toEqual([]);
  await expect(page.getByRole('region', { name: 'Children' })).toContainText(permissions);
  await context.close();
});
