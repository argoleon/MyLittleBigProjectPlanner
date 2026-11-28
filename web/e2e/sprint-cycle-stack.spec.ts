/**
 * A sprint's cycle on the production stack, nothing mocked (L09-US02): three
 * tickets planned in a sprint of a Scrum board are started from the
 * backlog, followed on the active sprint's board where one is moved to
 * Done, then the sprint is closed and its two unfinished tickets go to the
 * next sprint; the closed sprint keeps its committed and completed scope.
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

/**
 * Through the API, as the admin signed in on `page`: the project `key` with
 * three stories, a Scrum board on it with two sprints, the stories planned
 * in the first one; the board's id.
 */
async function plannedBoard(page: Page, key: string): Promise<string> {
  return page.evaluate(async (key) => {
    const session = (await (await fetch('/api/v1/auth/session')).json()) as { csrfToken: string };
    const headers = { 'content-type': 'application/json', 'x-csrf-token': session.csrfToken };
    const post = async (url: string, body: object, extra: Record<string, string> = {}) => {
      const response = await fetch(url, { method: 'POST', headers: { ...headers, ...extra }, body: JSON.stringify(body) });
      if (response.status >= 300) throw new Error(`${url}: ${response.status} ${await response.text()}`);
      return response.status === 204 ? null : ((await response.json()) as { id: string });
    };
    const project = await post('/api/v1/nodes', { type: 'project', name: `Sprint project ${key}`, key });
    const form = (await (await fetch(`/api/v1/projects/${key}/issue-form`)).json()) as { types: { id: string; key: string }[] };
    const story = form.types.find((type) => type.key === 'story');
    const issues: string[] = [];
    for (const summary of ['Migrate the sign-in to OIDC', 'Map the directory groups', 'Audit the sign-ins']) {
      const issue = await post(`/api/v1/projects/${key}/issues`, { summary, typeId: story?.id, priority: 'medium' }, { 'idempotency-key': crypto.randomUUID() });
      issues.push(issue!.id);
    }
    const board = await post('/api/v1/boards', { nodeId: project!.id, name: `Scrum ${key}`, kind: 'scrum', filter: `project = ${key}` });
    const first = await post(`/api/v1/boards/${board!.id}/sprints`, {});
    await post(`/api/v1/boards/${board!.id}/sprints`, {});
    for (const issue of issues) {
      await post(`/api/v1/boards/${board!.id}/backlog/moves`, { issueId: issue, sprintId: first!.id });
    }
    return board!.id;
  }, key);
}

function list(page: Page, index: number): Locator {
  return page.locator('app-backlog .list').nth(index);
}

function cardOf(page: Page, key: string): Locator {
  return page.locator('app-board-card', { has: page.locator('.key', { hasText: new RegExp(`^${key}$`) }) });
}

test('a sprint is started, followed on its board, and closed with its unfinished tickets moved on', async ({ page }) => {
  test.setTimeout(120_000);
  // Unique per run: the stack keeps its data.
  const run = Date.now().toString(36).toUpperCase().slice(-5);
  const key = `SY${run}`;
  await signIn(page, adminEmail!, adminSecret!);
  await page.goto('/');
  const board = await plannedBoard(page, key);

  // No active sprint: the board opens on its backlog; the first sprint starts there.
  await page.goto(`/boards/${board}`);
  await expect(page.locator('app-backlog .list .name')).toHaveText([`${key} Sprint 1`, `${key} Sprint 2`, 'Backlog']);
  await expect(list(page, 0).locator('.key')).toHaveText([`${key}-1`, `${key}-2`, `${key}-3`]);
  await list(page, 0).getByRole('button', { name: 'Start the sprint' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('heading')).toHaveText('Start the sprint');
  await dialog.getByRole('button', { name: 'Start' }).click();

  // The active sprint's board: its tickets in their columns, under its header.
  await expect(page.locator('.sprint-name')).toHaveText(`${key} Sprint 1`);
  await expect(page.locator('.sprint-left')).toHaveText('13 days left');
  await expect(page.locator('.column-name')).toHaveText(['To do 3', 'In progress 0', 'Done 0']);
  expect(await accessibilityViolations(page)).toEqual([]);

  // One ticket to Done, through its menu.
  await cardOf(page, `${key}-1`).getByRole('button', { name: `Move ${key}-1` }).click();
  await page.getByRole('menuitem', { name: 'Move to Done' }).click();
  await expect(page.locator('.column-name')).toHaveText(['To do 2', 'In progress 0', 'Done 1']);

  // Closed: the two others go to the next sprint.
  await page.getByRole('button', { name: 'Close the sprint' }).click();
  const close = page.getByRole('dialog');
  await expect(close.locator('.summary')).toHaveText('1 tickets done, 2 unfinished.');
  await expect(close.getByRole('combobox', { name: 'Sprint' })).toHaveText(`${key} Sprint 2`);
  await close.getByRole('button', { name: 'Close the sprint' }).click();
  await expect(page.locator('app-backlog .list .name')).toHaveText([`${key} Sprint 2`, 'Backlog']);
  await expect(list(page, 0).locator('.key')).toHaveText([`${key}-2`, `${key}-3`]);
  await expect(list(page, 1).locator('.key')).toHaveCount(0);

  // The closed sprint kept its scope.
  await page.getByRole('button', { name: 'Closed sprints' }).click();
  await expect(page.locator('.closed-sprint .meta')).toHaveText('0 of 0 points completed · 1 of 3 committed tickets');
  await page.getByRole('button', { name: `See the scope of ${key} Sprint 1` }).click();
  await expect(page.getByRole('dialog').locator('tr.mat-mdc-row')).toHaveCount(3);
});
