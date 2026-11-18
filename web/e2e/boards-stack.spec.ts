/**
 * The Kanban board on the production stack, nothing mocked (L06-US05): the
 * tenant admin creates a board on a project from its filter, through the
 * screens; a card dragged from To do to In progress runs "Start" and the
 * ticket's status changes; dragged back to To do, where no transition
 * leads, it returns to its column and the reason shows.
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

/** Through the API, as the admin signed in on `page`: the project `key` with two stories; its id. */
async function project(page: Page, key: string): Promise<string> {
  return page.evaluate(async (key) => {
    const session = (await (await fetch('/api/v1/auth/session')).json()) as { csrfToken: string };
    const headers = { 'content-type': 'application/json', 'x-csrf-token': session.csrfToken };
    const created = await fetch('/api/v1/nodes', {
      method: 'POST',
      headers,
      body: JSON.stringify({ type: 'project', name: `Board project ${key}`, key }),
    });
    if (created.status !== 201) throw new Error(`project: ${created.status} ${await created.text()}`);
    const id = ((await created.json()) as { id: string }).id;
    const form = (await (await fetch(`/api/v1/projects/${key}/issue-form`)).json()) as { types: { id: string; key: string }[] };
    const story = form.types.find((type) => type.key === 'story');
    for (const summary of ['Migrate the sign-in to OIDC', 'Map the directory groups']) {
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

async function statusOf(page: Page, key: string): Promise<string> {
  return page.evaluate(async (key) => {
    const issue = (await (await fetch(`/api/v1/issues/${key}`)).json()) as { status: { key: string } };
    return issue.status.key;
  }, key);
}

function column(page: Page, index: number): Locator {
  return page.locator('.cell').nth(index);
}

function cardOf(page: Page, key: string): Locator {
  return page.locator('app-board-card', { has: page.locator('.key', { hasText: new RegExp(`^${key}$`) }) });
}

/** Drags `source` over `target` with the mouse, as the CDK expects: pressed, moved past its threshold, released. */
async function drag(page: Page, source: Locator, target: Locator) {
  const from = (await source.locator('.key').boundingBox())!;
  const to = (await target.boundingBox())!;
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
  await page.mouse.down();
  await page.mouse.move(from.x + from.width / 2 + 10, from.y + from.height / 2 + 10, { steps: 5 });
  await page.mouse.move(to.x + to.width / 2, to.y + 40, { steps: 20 });
  await page.mouse.up();
}

test('a board made from a filter moves a card through the workflow and puts back a forbidden move', async ({ page }) => {
  test.setTimeout(120_000);
  // Unique per run: the stack keeps its data.
  const run = Date.now().toString(36).toUpperCase().slice(-5);
  const key = `KB${run}`;
  await signIn(page, adminEmail!, adminSecret!);
  await page.goto('/');
  const id = await project(page, key);

  // From the project's page to its boards, then a new one on it.
  await page.goto(`/nodes/${id}`);
  await page.getByRole('link', { name: 'Boards' }).click();
  await expect(page.getByText(`Boards of Board project ${key}`)).toBeVisible();
  await page.getByRole('button', { name: 'New board' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('combobox', { name: 'Portfolio, program or project' })).toContainText(`Board project ${key} (${key})`);
  await dialog.getByLabel('Name').fill(`Board ${key}`);
  await dialog.getByLabel('Filter').fill(`project = ${key}`);
  await dialog.getByRole('button', { name: 'Create the board' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Board ${key}`);
  await expect(page.locator('.column-name')).toHaveText(['To do 2', 'In progress 0', 'Done 0']);
  await expect(column(page, 0).locator('.key')).toHaveText([`${key}-1`, `${key}-2`].sort());
  expect(await accessibilityViolations(page)).toEqual([]);

  // To do → In progress: "Start".
  await drag(page, cardOf(page, `${key}-1`), column(page, 1));
  await expect(column(page, 1).locator('.key')).toHaveText([`${key}-1`]);
  await expect(page.locator('.column-name')).toHaveText(['To do 1', 'In progress 1', 'Done 0']);
  await expect.poll(() => statusOf(page, `${key}-1`)).toBe('in_progress');

  // Back to To do: no transition leads there.
  await drag(page, cardOf(page, `${key}-1`), column(page, 0));
  await expect(page.getByRole('alert')).toContainText(`${key}-1 went back: no transition of its workflow leads from In progress to To do.`);
  await expect(column(page, 1).locator('.key')).toHaveText([`${key}-1`]);
  await expect(column(page, 0).locator('.key')).toHaveText([`${key}-2`]);
  expect(await statusOf(page, `${key}-1`)).toBe('in_progress');
});
