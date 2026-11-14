/**
 * Project configurations on the production stack, nothing mocked
 * (L04-US01 to US05): the tenant admin creates a configuration, moves two projects
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

test('a custom field with its rules is published, then a value of it is deactivated, not removed', async ({ browser }) => {
  test.setTimeout(120_000);
  const run = Date.now().toString(36).toUpperCase().slice(-6);
  const config = `Operations ${run}`;

  const admin = await signIn(browser, adminEmail!, adminSecret!);
  await admin.goto('/admin/configurations');
  await admin.getByRole('button', { name: 'New configuration' }).click();
  const dialog = admin.getByRole('dialog', { name: 'New project configuration' });
  await expect(dialog.getByLabel('Name (Français)')).toBeFocused();
  await dialog.getByLabel('Name (Français)').fill(`Exploitation ${run}`);
  await dialog.getByLabel('Name (English)').fill(config);
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(admin.getByRole('heading', { level: 1 })).toHaveText(config);
  await admin.getByRole('tab', { name: 'Fields' }).click();

  // The system fields of every configuration.
  await expect(admin.locator('app-fields-tab code')).toHaveText(['story_points', 'remaining']);
  await expect(admin.locator('app-fields-tab .system')).toHaveCount(2);

  // Application, a single select on bugs, required at their creation.
  await admin.getByRole('button', { name: 'New field' }).click();
  const panel = admin.getByRole('region', { name: 'New field' });
  await panel.getByLabel('Label (Français)').fill('Application');
  await panel.getByLabel('Label (English)').fill('Application');
  await panel.getByLabel('Identifier').fill('application');
  await panel.getByRole('combobox', { name: 'Type', exact: true }).focus();
  await admin.keyboard.press('Enter');
  await admin.getByRole('option', { name: 'Single select' }).click();
  await panel.getByLabel('Value 1 (Français)').fill('Annuaire');
  await panel.getByLabel('Value 1 (English)').fill('Directory');
  await panel.getByRole('button', { name: 'Add a value' }).click();
  await panel.getByLabel('Value 2 (Français)').fill('Messagerie');
  await panel.getByLabel('Value 2 (English)').fill('Mail');
  await panel.getByRole('combobox', { name: 'Issue types of the field' }).focus();
  await admin.keyboard.press('Enter');
  await admin.getByRole('option', { name: 'Bug', exact: true }).click();
  await admin.keyboard.press('Escape');
  await panel.getByRole('button', { name: 'Bug, Creation: Visible' }).click();
  await expect(panel.getByRole('button', { name: 'Bug, Creation: Required' })).toBeVisible();
  await panel.getByRole('button', { name: 'Save' }).click();
  await expect(admin.getByRole('status')).toHaveText('Field “Application” saved in the draft.');
  await expect(admin.locator('app-fields-tab code')).toHaveText(['story_points', 'remaining', 'application']);
  expect(await accessibilityViolations(admin), 'field panel').toEqual([]);

  // Published: version 2, with the field and its rule.
  await admin.getByRole('button', { name: 'Publish the changes' }).click();
  const publish = admin.getByRole('dialog', { name: `Publish “${config}”?` });
  await publish.getByRole('button', { name: 'Publish' }).click();
  await expect(admin.getByText('Version 2 is published.', { exact: true })).toBeVisible();
  await admin.reload();
  await admin.getByRole('button', { name: 'Edit Application' }).click();
  const edit = admin.getByRole('region', { name: 'Edit “Application”' });
  await expect(edit.getByRole('button', { name: 'Bug, Creation: Required' })).toBeVisible();

  // Mail is published: removing it deactivates it, and it stays.
  await edit.getByRole('button', { name: 'Actions of value 2' }).click();
  await admin.getByRole('menuitem', { name: 'Deactivate' }).click();
  await edit.getByRole('button', { name: 'Save' }).click();
  await expect(admin.getByRole('status')).toHaveText('Field “Application” saved in the draft.');
  await expect(edit.getByText('Deactivated: no longer offered, kept on the tickets that have it.')).toBeVisible();
  await expect(edit.getByLabel('Value 2 (English)')).toHaveValue('Mail');

  await admin.context().close();
});

test('a transition restricted to a role, requiring a field and assigning its taker is published with its rules', async ({ browser }) => {
  test.setTimeout(120_000);
  const run = Date.now().toString(36).toUpperCase().slice(-6);
  const config = `Review ${run}`;

  const admin = await signIn(browser, adminEmail!, adminSecret!);
  await admin.goto('/admin/configurations');
  await admin.getByRole('button', { name: 'New configuration' }).click();
  const dialog = admin.getByRole('dialog', { name: 'New project configuration' });
  await expect(dialog.getByLabel('Name (Français)')).toBeFocused();
  await dialog.getByLabel('Name (Français)').fill(`Revue ${run}`);
  await dialog.getByLabel('Name (English)').fill(config);
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(admin.getByRole('heading', { level: 1 })).toHaveText(config);
  await admin.getByRole('tab', { name: 'Workflows' }).click();
  const validate = admin.getByRole('row', { name: /Validate/ });
  await expect(validate).toContainText('Open to everyone');

  // Validate: one role, the remaining work filled in, assigned to whoever validates.
  await admin.getByRole('button', { name: 'Edit Validate' }).click();
  const edit = admin.getByRole('dialog', { name: 'Change the transition' });
  await edit.getByRole('combobox', { name: 'Roles' }).click();
  const role = admin.getByRole('listbox', { name: 'Roles' }).getByRole('option').first();
  const roleName = (await role.textContent())!.trim();
  await role.click();
  // A multiple select stays open: Tab closes it and leaves the dialog open.
  await admin.keyboard.press('Tab');
  await expect(admin.getByRole('listbox')).toHaveCount(0);
  await edit.getByRole('combobox', { name: 'Fields that must be filled in' }).click();
  await admin.getByRole('option', { name: 'Remaining work' }).click();
  await admin.keyboard.press('Tab');
  await expect(admin.getByRole('listbox')).toHaveCount(0);
  await edit.getByRole('button', { name: 'Add a post-function' }).click();
  await expect(edit.getByRole('combobox', { name: 'Assign to' })).toContainText('Whoever takes the transition');
  expect(await accessibilityViolations(admin), 'transition rules').toEqual([]);
  await edit.getByRole('button', { name: 'Save' }).click();
  await expect(admin.getByRole('status')).toHaveText('Transition saved in “Standard workflow”.');
  await expect(validate).toContainText(`Only: ${roleName}`);
  await expect(validate).toContainText('Requires Remaining work');
  await expect(validate).toContainText('Assigns to whoever takes it');

  // Published: the projects' version has them.
  await admin.getByRole('button', { name: 'Publish the changes' }).click();
  const publish = admin.getByRole('dialog', { name: `Publish “${config}”?` });
  await publish.getByRole('button', { name: 'Publish' }).click();
  await expect(admin.getByText('Version 2 is published.', { exact: true })).toBeVisible();
  await admin.reload();
  await expect(admin.getByRole('row', { name: /Validate/ })).toContainText('Requires Remaining work');

  await admin.context().close();
});
