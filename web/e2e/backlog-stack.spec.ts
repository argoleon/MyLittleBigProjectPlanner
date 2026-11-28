/**
 * The Scrum backlog on the production stack, nothing mocked (L09-US01): the
 * tenant admin creates a Scrum board on a project through the screens, then
 * a sprint named after the project; a ticket dragged from the backlog into
 * the sprint is planned there, another is sent with the keyboard through its
 * menu and moved to the top; the order is kept when the page is read again,
 * and the query language finds the sprint's tickets.
 *
 * Runs with `just compose-e2e` only: deploy/tests/e2e.sh creates the admin
 * and passes E2E_ADMIN_EMAIL and E2E_ADMIN_SECRET.
 */
import { type Locator, type Page, expect, test } from '@playwright/test';

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

/** Through the API, as the admin signed in on `page`: the project `key` with three stories; its id. */
async function project(page: Page, key: string): Promise<string> {
  return page.evaluate(async (key) => {
    const session = (await (await fetch('/api/v1/auth/session')).json()) as { csrfToken: string };
    const headers = { 'content-type': 'application/json', 'x-csrf-token': session.csrfToken };
    const created = await fetch('/api/v1/nodes', {
      method: 'POST',
      headers,
      body: JSON.stringify({ type: 'project', name: `Scrum project ${key}`, key }),
    });
    if (created.status !== 201) throw new Error(`project: ${created.status} ${await created.text()}`);
    const id = ((await created.json()) as { id: string }).id;
    const form = (await (await fetch(`/api/v1/projects/${key}/issue-form`)).json()) as { types: { id: string; key: string }[] };
    const story = form.types.find((type) => type.key === 'story');
    for (const summary of ['Migrate the sign-in to OIDC', 'Map the directory groups', 'Audit the sign-ins']) {
      const response = await fetch(`/api/v1/projects/${key}/issues`, {
        method: 'POST',
        headers: { ...headers, 'idempotency-key': crypto.randomUUID() },
        body: JSON.stringify({ summary, typeId: story?.id, priority: 'medium' }),
      });
      if (response.status !== 201) throw new Error(`ticket: ${response.status}`);
    }
    return id;
  }, key);
}

/** The keys the query finds, as the admin signed in on `page`. */
async function found(page: Page, query: string): Promise<string[]> {
  return page.evaluate(async (query) => {
    const session = (await (await fetch('/api/v1/auth/session')).json()) as { csrfToken: string };
    const response = await fetch('/api/v1/search', {
      method: 'POST',
      headers: { 'content-type': 'application/json', 'x-csrf-token': session.csrfToken },
      body: JSON.stringify({ query }),
    });
    const page = (await response.json()) as { items: { key: string }[] };
    return page.items.map((item) => item.key);
  }, query);
}

function list(page: Page, index: number): Locator {
  return page.locator('app-backlog .list').nth(index);
}

function rowOf(page: Page, key: string): Locator {
  return page.locator('app-backlog-row', { has: page.locator('.key', { hasText: new RegExp(`^${key}$`) }) });
}

/** Drags the handle of `source` over `target` with the mouse, as the CDK expects. */
async function drag(page: Page, source: Locator, target: Locator) {
  await source.locator('.handle').scrollIntoViewIfNeeded();
  const from = (await source.locator('.handle').boundingBox())!;
  const to = (await target.boundingBox())!;
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
  await page.mouse.down();
  await page.mouse.move(from.x + from.width / 2 + 10, from.y + from.height / 2 + 10, { steps: 5 });
  await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 20 });
  await page.mouse.up();
}

test('a Scrum board plans tickets from its backlog into a sprint and keeps their rank', async ({ page }) => {
  test.setTimeout(120_000);
  // Unique per run: the stack keeps its data.
  const run = Date.now().toString(36).toUpperCase().slice(-5);
  const key = `SC${run}`;
  await signIn(page, adminEmail!, adminSecret!);
  await page.goto('/');
  const id = await project(page, key);

  // A Scrum board on the project, through its boards.
  await page.goto(`/nodes/${id}`);
  await page.locator('.actions').getByRole('link', { name: 'Boards' }).click();
  await page.getByRole('button', { name: 'New board' }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('combobox', { name: 'Kind' }).click();
  await page.getByRole('option', { name: 'Scrum' }).click();
  await dialog.getByLabel('Name').fill(`Scrum ${key}`);
  await dialog.getByLabel('Filter').fill(`project = ${key}`);
  await dialog.getByRole('button', { name: 'Create the board' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Scrum ${key}`);
  // No sprint yet: the backlog only, the last changed first.
  await expect(page.locator('app-backlog .list .name')).toHaveText(['Backlog']);
  await expect(list(page, 0).locator('.key')).toHaveText([`${key}-3`, `${key}-2`, `${key}-1`]);

  // A sprint named after the project.
  await page.getByRole('button', { name: 'Create a sprint' }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Create' }).click();
  await expect(page.locator('app-backlog .list .name')).toHaveText([`${key} Sprint 1`, 'Backlog']);
  await expect(list(page, 0).locator('.empty')).toBeVisible();
  expect(await accessibilityViolations(page)).toEqual([]);

  // Dragged from the backlog into the sprint.
  await drag(page, rowOf(page, `${key}-1`), list(page, 0).locator('.rows'));
  await expect(list(page, 0).locator('.key')).toHaveText([`${key}-1`]);
  await expect(list(page, 1).locator('.key')).toHaveText([`${key}-3`, `${key}-2`]);

  // With the keyboard: to the sprint, then on top of it.
  await rowOf(page, `${key}-2`).getByRole('button', { name: `Plan ${key}-2` }).click();
  await page.getByRole('menuitem', { name: `Move to ${key} Sprint 1` }).click();
  await expect(list(page, 0).locator('.key')).toHaveText([`${key}-1`, `${key}-2`]);
  await rowOf(page, `${key}-2`).getByRole('button', { name: `Plan ${key}-2` }).click();
  await page.getByRole('menuitem', { name: 'Move to the top' }).click();
  await expect(list(page, 0).locator('.key')).toHaveText([`${key}-2`, `${key}-1`]);

  // Read again: the order is kept; the query language finds the sprint's tickets.
  await page.reload();
  await expect(list(page, 0).locator('.key')).toHaveText([`${key}-2`, `${key}-1`]);
  await expect(list(page, 1).locator('.key')).toHaveText([`${key}-3`]);
  expect((await found(page, `project = ${key} AND sprint IN openSprints() ORDER BY key`)).length).toBe(2);
  expect(await found(page, `project = ${key} AND sprint IS EMPTY`)).toEqual([`${key}-3`]);
});
