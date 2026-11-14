/**
 * Project configurations on the production stack, nothing mocked
 * (L04-US01): the tenant admin creates a configuration, moves two projects
 * to it from their settings, sees it shared by both, then detaches one,
 * which gets its own copy.
 *
 * Runs with `just compose-e2e` only: deploy/tests/e2e.sh creates the admin
 * and passes E2E_ADMIN_EMAIL and E2E_ADMIN_SECRET.
 */
import { type Browser, type Page, expect, test } from '@playwright/test';

import { accessibilityViolations } from './fixtures';

const adminEmail = process.env['E2E_ADMIN_EMAIL'];
const adminSecret = process.env['E2E_ADMIN_SECRET'];

test.skip(!adminEmail || !adminSecret, 'needs the compose stack (just compose-e2e)');

/** A browser of their own for the member `email`, signed in, in English. */
async function signIn(browser: Browser, email: string, secret: string): Promise<Page> {
  const context = await browser.newContext();
  await context.addInitScript(() => localStorage.setItem('mlbpp.language', 'en'));
  const page = await context.newPage();
  await page.goto('/sign-in');
  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password').fill(secret);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');
  return page;
}

/** Creates a project at the root of the hierarchy; the URL of its settings. */
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

/** Moves the project whose settings are at `url` to the configuration `name`. */
async function moveTo(page: Page, url: string, name: string) {
  await page.goto(`${url}/configuration`);
  await expect(page.locator('.uses')).toContainText('Default configuration');
  await page.getByRole('combobox', { name: 'Configuration' }).click();
  await page.getByRole('option', { name, exact: true }).click();
  await page.getByRole('button', { name: 'Move the project' }).click();
  await expect(page.getByRole('status')).toHaveText(`The project now uses “${name}”.`);
}

test('a configuration is shared by two projects, then one of them detaches with its own copy', async ({ browser }) => {
  test.setTimeout(120_000);
  const run = Date.now().toString(36).toUpperCase().slice(-6);
  const config = `Software ${run}`;
  const [first, second] = [`Platform ${run}`, `Portal ${run}`];

  const admin = await signIn(browser, adminEmail!, adminSecret!);
  const firstUrl = await project(admin, first, `P${run}`);
  const secondUrl = await project(admin, second, `Q${run}`);

  // Created from the administration, with a name in both languages.
  await admin.goto('/admin/configurations');
  await admin.getByRole('button', { name: 'New configuration' }).click();
  const dialog = admin.getByRole('dialog', { name: 'New project configuration' });
  // Opened: its first field has the focus, which would otherwise move while typing.
  await expect(dialog.getByLabel('Name (Français)')).toBeFocused();
  await dialog.getByLabel('Name (Français)').fill(`Logiciel ${run}`);
  await dialog.getByLabel('Name (English)').fill(config);
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(admin.getByRole('heading', { level: 1 })).toHaveText(config);
  await expect(admin.getByText('Used by no project')).toBeVisible();

  // Both projects move to it: shared by two.
  await moveTo(admin, firstUrl, config);
  await moveTo(admin, secondUrl, config);
  await admin.getByRole('link', { name: config }).click();
  await expect(admin.getByText('Shared by 2 projects')).toBeVisible();
  await admin.getByRole('tab', { name: 'Projects (2)' }).click();
  await expect(admin.getByRole('row')).toHaveCount(3);
  expect(await accessibilityViolations(admin), 'projects of a configuration').toEqual([]);

  // The first one detaches: its own copy, the original keeps the second.
  await admin.getByRole('button', { name: `Detach ${first}` }).click();
  const detach = admin.getByRole('dialog', { name: `Detach ${first}` });
  await expect(detach.getByLabel('Name (English)')).toHaveValue(`${config} (P${run})`);
  await detach.getByRole('button', { name: 'Detach' }).click();
  await expect(admin.getByRole('status')).toHaveText(`${first} now uses its own configuration “${config} (P${run})”.`);
  await expect(admin.getByRole('tab', { name: 'Project (1)' })).toBeVisible();
  await expect(admin.getByRole('row')).toHaveCount(2);
  await admin.goto(`${firstUrl}/configuration`);
  await expect(admin.locator('.uses')).toHaveText(`This project uses ${config} (P${run})`);
  await expect(admin.getByText('Used by 1 project')).toBeVisible();

  await admin.context().close();
});

test('an issue type added to the draft reaches the projects once published', async ({ browser }) => {
  test.setTimeout(120_000);
  const run = Date.now().toString(36).toUpperCase().slice(-6);
  const config = `Support ${run}`;
  const identifiers = (page: Page) => page.locator('app-types-tab code');

  const admin = await signIn(browser, adminEmail!, adminSecret!);
  await admin.goto('/admin/configurations');
  await admin.getByRole('button', { name: 'New configuration' }).click();
  const dialog = admin.getByRole('dialog', { name: 'New project configuration' });
  await expect(dialog.getByLabel('Name (Français)')).toBeFocused();
  await dialog.getByLabel('Name (Français)').fill(`Assistance ${run}`);
  await dialog.getByLabel('Name (English)').fill(config);
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(admin.getByRole('heading', { level: 1 })).toHaveText(config);

  // The default types, Epic at the top and Sub-task at the bottom.
  await expect(identifiers(admin)).toHaveText(['epic', 'story', 'task', 'bug', 'subtask']);

  // A new type under Epic: it goes to the draft.
  await admin.getByRole('button', { name: 'New type' }).click();
  const panel = admin.getByRole('region', { name: 'New issue type' });
  await panel.getByLabel('Label (Français)').fill('Incident');
  await panel.getByLabel('Label (English)').fill('Incident');
  await panel.getByLabel('Identifier').fill('incident');
  // With the keyboard: the empty field's label lies over the middle of the select.
  await panel.getByRole('combobox', { name: 'Allowed parent types' }).focus();
  await admin.keyboard.press('Enter');
  await admin.getByRole('option', { name: 'Epic', exact: true }).click();
  await admin.keyboard.press('Escape');
  await panel.getByRole('button', { name: 'Save' }).click();
  await expect(admin.getByRole('status')).toHaveText('Type “Incident” saved in the draft.');
  await expect(admin.getByText('Unpublished draft · 1 change')).toBeVisible();
  await expect(identifiers(admin)).toHaveText(['epic', 'story', 'task', 'bug', 'subtask', 'incident']);
  expect(await accessibilityViolations(admin), 'types in the draft').toEqual([]);

  // Published: version 2, with the new type.
  await admin.getByRole('button', { name: 'Publish the changes' }).click();
  const publish = admin.getByRole('dialog', { name: `Publish “${config}”?` });
  await publish.getByRole('button', { name: 'Publish' }).click();
  await expect(admin.getByText('Version 2 is published.', { exact: true })).toBeVisible();
  await admin.reload();
  await expect(identifiers(admin)).toHaveText(['epic', 'story', 'task', 'bug', 'subtask', 'incident']);
  await expect(admin.getByText(/Unpublished draft/)).toHaveCount(0);

  await admin.context().close();
});

test('a status nothing leads to keeps the workflow from being published until a transition does', async ({ browser }) => {
  test.setTimeout(120_000);
  const run = Date.now().toString(36).toUpperCase().slice(-6);
  const config = `Delivery ${run}`;

  const admin = await signIn(browser, adminEmail!, adminSecret!);
  await admin.goto('/admin/configurations');
  await admin.getByRole('button', { name: 'New configuration' }).click();
  const dialog = admin.getByRole('dialog', { name: 'New project configuration' });
  await expect(dialog.getByLabel('Name (Français)')).toBeFocused();
  await dialog.getByLabel('Name (Français)').fill(`Livraison ${run}`);
  await dialog.getByLabel('Name (English)').fill(config);
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(admin.getByRole('heading', { level: 1 })).toHaveText(config);
  await admin.getByRole('tab', { name: 'Workflows' }).click();

  // The standard workflow, followed by every default type.
  await expect(admin.getByRole('button', { name: /Epic, Story, Task, Bug, Sub-task/ })).toBeVisible();
  await expect(admin.getByRole('img', { name: /Graph of Standard workflow/ })).toBeVisible();
  expect(await accessibilityViolations(admin), 'standard workflow').toEqual([]);

  // A new status: nothing leads to it yet.
  await admin.getByRole('button', { name: 'Add a status' }).click();
  const status = admin.getByRole('dialog', { name: 'New status' });
  await expect(status.getByLabel('Label (Français)')).toBeFocused();
  await status.getByLabel('Label (Français)').fill('En attente');
  await status.getByLabel('Label (English)').fill('On hold');
  await status.getByLabel('Identifier').fill('on_hold');
  await status.getByRole('combobox', { name: 'Category' }).focus();
  await admin.keyboard.press('Enter');
  await admin.getByRole('option', { name: 'In progress', exact: true }).click();
  await status.getByRole('button', { name: 'Save' }).click();
  await expect(admin.getByRole('status')).toHaveText('Status saved in “Standard workflow”.');
  await expect(admin.locator('.problems')).toContainText('nothing leads to “On hold” from the initial status.');

  // Publishing waits for it.
  await admin.getByRole('button', { name: 'Publish the changes' }).click();
  const publish = admin.getByRole('dialog', { name: `Publish “${config}”?` });
  await expect(publish.getByText('Workflows of the draft to fix first:')).toBeVisible();
  await expect(publish.getByRole('button', { name: 'Publish' })).toBeDisabled();
  await admin.keyboard.press('Escape');

  // A transition to it, from In progress: ready, and published.
  await admin.getByRole('button', { name: 'Add a transition' }).click();
  const transition = admin.getByRole('dialog', { name: 'New transition' });
  await expect(transition.getByLabel('Transition (Français)')).toBeFocused();
  await transition.getByLabel('Transition (Français)').fill('Mettre en attente');
  await transition.getByLabel('Transition (English)').fill('Put on hold');
  await transition.getByRole('combobox', { name: 'From' }).focus();
  await admin.keyboard.press('Enter');
  await admin.getByRole('listbox', { name: 'From' }).getByRole('option', { name: 'In progress', exact: true }).click();
  await transition.getByRole('combobox', { name: 'To' }).focus();
  await admin.keyboard.press('Enter');
  await admin.getByRole('listbox', { name: 'To' }).getByRole('option', { name: 'On hold', exact: true }).click();
  await transition.getByRole('button', { name: 'Save' }).click();
  await expect(admin.getByRole('status')).toHaveText('Transition saved in “Standard workflow”.');
  await expect(admin.locator('.problems')).toHaveCount(0);
  await admin.getByRole('button', { name: 'Publish the changes' }).click();
  await publish.getByRole('button', { name: 'Publish' }).click();
  await expect(admin.getByText('Version 2 is published.', { exact: true })).toBeVisible();

  await admin.context().close();
});
