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

test('a story is edited with the version read, then moved with its sub-task; its former key still opens it', async ({ page }) => {
  test.setTimeout(120_000);
  const run = Date.now().toString(36).toUpperCase().slice(-5);
  const [from, to] = [`MA${run}`.slice(0, 10), `MB${run}`.slice(0, 10)];
  await signIn(page, adminEmail!, adminSecret!);
  const url = await project(page, `Mail ${run}`, from);
  await project(page, `Billing ${run}`, to);

  // A story with a sub-task, from the project's page.
  await page.goto(url);
  await page.getByRole('button', { name: 'New ticket' }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('combobox', { name: 'Type' }).click();
  await page.getByRole('option', { name: 'Story', exact: true }).click();
  await dialog.getByLabel('Summary').fill('Archive old mailboxes');
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(page).toHaveURL(new RegExp(`/issues/${from}-1$`));
  await page.getByRole('button', { name: 'Add a sub-task' }).click();
  await dialog.getByLabel('Summary').fill('List the mailboxes');
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(page).toHaveURL(new RegExp(`/issues/${from}-2$`));

  // Edited: the summary and the priority.
  await page.goto(`/issues/${from}-1`);
  await page.getByRole('button', { name: 'Edit' }).click();
  await dialog.getByLabel('Summary').fill('Archive the mailboxes of former staff');
  await dialog.getByRole('combobox', { name: 'Priority' }).click();
  await page.getByRole('option', { name: 'Highest', exact: true }).click();
  await dialog.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Archive the mailboxes of former staff');
  await expect(page.locator('.detail', { hasText: 'Priority' })).toContainText('Highest');

  // Moved, with its sub-task: the same configuration, nothing to map.
  await page.getByRole('button', { name: 'Move' }).click();
  await dialog.getByRole('combobox', { name: 'Target project' }).click();
  await page.getByRole('option', { name: `Billing ${run} (${to})` }).click();
  await expect(dialog.locator('.tickets li')).toHaveText([
    `${from}-1 Archive the mailboxes of former staff`,
    `${from}-2 List the mailboxes`,
  ]);
  await expect(dialog.getByText('Both projects use the same configuration')).toBeVisible();
  expect(await accessibilityViolations(page)).toEqual([]);
  await dialog.getByRole('button', { name: 'Move' }).click();
  await expect(page).toHaveURL(new RegExp(`/issues/${to}-1$`));
  await expect(page.locator('.identity')).toContainText(`${to}-1`);

  // Former keys still open the tickets, under their new keys.
  await page.goto(`/issues/${from}-2`);
  await expect(page.locator('.identity')).toContainText(`${to}-2`);
  await expect(page.getByRole('navigation', { name: 'Where the ticket belongs' })).toContainText(
    `${to}-1 Archive the mailboxes of former staff`,
  );
});

test('a ticket blocks one of another project, each side reading it its way; a story changes epic', async ({ page }) => {
  test.setTimeout(120_000);
  const run = Date.now().toString(36).toUpperCase().slice(-5);
  const [one, two] = [`LA${run}`.slice(0, 10), `LB${run}`.slice(0, 10)];
  await signIn(page, adminEmail!, adminSecret!);
  const first = await project(page, `Links ${run}`, one);
  const second = await project(page, `Other ${run}`, two);
  const dialog = page.getByRole('dialog');
  const create = async (url: string, type: string, summary: string) => {
    await page.goto(url);
    await page.getByRole('button', { name: 'New ticket' }).click();
    await dialog.getByRole('combobox', { name: 'Type' }).click();
    await page.getByRole('option', { name: type, exact: true }).click();
    await dialog.getByLabel('Summary').fill(summary);
    await dialog.getByRole('button', { name: 'Create' }).click();
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(summary);
  };
  await create(first, 'Epic', `Sign-in ${run}`);
  await create(first, 'Epic', `Security ${run}`);
  await create(first, 'Story', `Migrate to OIDC ${run}`);
  await create(second, 'Story', `Portal forms ${run}`);

  // From the story: it blocks the ticket of the other project.
  await page.goto(`/issues/${one}-3`);
  await page.getByRole('button', { name: 'Link a ticket' }).click();
  await dialog.getByRole('combobox', { name: 'This ticket' }).click();
  await page.getByRole('option', { name: 'Blocks', exact: true }).click();
  await dialog.getByRole('combobox', { name: 'Ticket', exact: true }).fill(`Portal forms ${run}`);
  await page.getByRole('option', { name: new RegExp(`${two}-1`) }).click();
  await dialog.getByRole('button', { name: 'Link', exact: true }).click();
  const lines = page.locator('app-issue-relations .line');
  await expect(lines).toHaveCount(1);
  await expect(lines.locator('.relation')).toHaveText('Blocks');
  await expect(lines.getByRole('link')).toHaveText(`${two}-1`);
  expect(await accessibilityViolations(page)).toEqual([]);

  // The other side reads it the other way.
  await lines.getByRole('link', { name: `${two}-1` }).click();
  await expect(page.locator('app-issue-relations .line .relation')).toHaveText('Blocked by');

  // The story goes under an epic, then another.
  for (const epic of [`${one}-1`, `${one}-2`]) {
    await page.goto(`/issues/${one}-3`);
    await page.getByRole('button', { name: 'Change parent' }).click();
    await dialog.getByRole('combobox', { name: 'Parent ticket' }).fill(epic);
    await page.getByRole('option', { name: new RegExp(epic) }).click();
    await dialog.getByRole('button', { name: 'Save' }).click();
    await expect(page.getByRole('navigation', { name: 'Where the ticket belongs' })).toContainText(epic);
  }
  await page.goto(`/issues/${one}-2`);
  await expect(lines.getByRole('link')).toHaveText(`${one}-3`);
  await expect(lines.locator('.relation')).toHaveText('Sub-task');
});

test('a story goes through the standard workflow from its page', async ({ page }) => {
  test.setTimeout(120_000);
  const run = Date.now().toString(36).toUpperCase().slice(-5);
  const code = `WF${run}`.slice(0, 10);
  await signIn(page, adminEmail!, adminSecret!);
  const url = await project(page, `Workflow ${run}`, code);
  await page.goto(url);
  await page.getByRole('button', { name: 'New ticket' }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('combobox', { name: 'Type' }).click();
  await page.getByRole('option', { name: 'Story', exact: true }).click();
  await dialog.getByLabel('Summary').fill(`Workflow ${run}`);
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(page).toHaveURL(new RegExp(`/issues/${code}-1$`));

  // To do: Start is the first transition, offered as a button.
  await expect(page.getByRole('button', { name: 'Status: To do. Change it' })).toBeVisible();
  expect(await accessibilityViolations(page)).toEqual([]);
  await page.getByRole('button', { name: 'Start', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Status: In progress. Change it' })).toBeVisible();

  // From the menu: Send to review, then Validate.
  for (const [transition, status] of [
    ['Send to review', 'In review'],
    ['Validate', 'Done'],
  ]) {
    await page.locator('button.status-menu').click();
    await page.getByRole('menuitem', { name: new RegExp(`^${transition}`) }).click();
    await expect(page.getByRole('button', { name: `Status: ${status}. Change it` }).or(page.locator('ui-status-chip', { hasText: status }))).toBeVisible();
  }
  // Read again: Done, stored.
  await page.reload();
  await expect(page.locator('app-issue-status')).toContainText('Done');
});
