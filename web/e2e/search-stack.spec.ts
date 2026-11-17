/**
 * Search on the production stack, nothing mocked (L06-US01): the tenant
 * admin builds a portfolio holding two projects, and a project beside it,
 * with tickets of several priorities; the search screen then finds the
 * tickets of the portfolio's projects, highest priority first, sorts them
 * by key from a column, and shows a refused query at its position.
 * Full text (L06-US03): words with typos find the tickets holding them,
 * those with the words in their title first.
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

/**
 * Through the API, as the admin signed in on `page`: the portfolio `portfolio`
 * with the projects `inside` under it, the project `outside` at the root, and
 * in each project a story per priority of `priorities`.
 */
async function build(page: Page, portfolio: string, inside: readonly string[], outside: string, priorities: readonly string[]) {
  await page.evaluate(
    async ({ portfolio, inside, outside, priorities }) => {
      const session = (await (await fetch('/api/v1/auth/session')).json()) as { csrfToken: string };
      const headers = { 'content-type': 'application/json', 'x-csrf-token': session.csrfToken };
      const node = async (body: object) => {
        const response = await fetch('/api/v1/nodes', { method: 'POST', headers, body: JSON.stringify(body) });
        if (response.status !== 201) throw new Error(`node: ${response.status} ${await response.text()}`);
        return ((await response.json()) as { id: string }).id;
      };
      const top = await node({ type: 'portfolio', name: `Portfolio ${portfolio}`, key: portfolio });
      for (const key of inside) await node({ type: 'project', name: `Project ${key}`, key, parentId: top });
      await node({ type: 'project', name: `Project ${outside}`, key: outside });
      for (const key of [...inside, outside]) {
        const form = (await (await fetch(`/api/v1/projects/${key}/issue-form`)).json()) as { types: { id: string; key: string }[] };
        const story = form.types.find((type) => type.key === 'story');
        for (const priority of priorities) {
          const response = await fetch(`/api/v1/projects/${key}/issues`, {
            method: 'POST',
            headers: { ...headers, 'idempotency-key': crypto.randomUUID() },
            body: JSON.stringify({ summary: `${priority} story of ${key}`, typeId: story?.id, priority }),
          });
          if (response.status !== 201) throw new Error(`ticket in ${key}: ${response.status}`);
        }
      }
    },
    { portfolio, inside: [...inside], outside, priorities: [...priorities] },
  );
}

test('the tickets under a portfolio are found by a query, sorted by priority, then by key', async ({ page }) => {
  test.setTimeout(120_000);
  // Unique per run: the stack keeps its data.
  const run = Date.now().toString(36).toUpperCase().slice(-5);
  const portfolio = `PF-${run}`;
  const [first, second, outside] = [`SA${run}`, `SB${run}`, `SO${run}`];
  await signIn(page, adminEmail!, adminSecret!);
  await page.goto('/');
  await build(page, portfolio, [first, second], outside, ['low', 'highest']);

  await page.getByRole('link', { name: 'Search' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Search');
  // The builder the first time: this query goes in the editor.
  await page.getByRole('radio', { name: 'Query' }).click();
  const field = page.getByRole('combobox', { name: 'Query' });
  await field.fill(`project IN descendantsOf("${portfolio}") ORDER BY priority DESC`);
  await field.press('Enter');
  await expect(page.getByText('Results: 4')).toBeVisible();
  const keys = page.locator('td.key a');
  await expect(keys).toHaveCount(4);
  // The highest first (one per project), the project outside never.
  await expect(page.locator('tr.mat-mdc-row').nth(0)).toContainText('highest story');
  await expect(page.locator('tr.mat-mdc-row').nth(1)).toContainText('highest story');
  await expect(page.getByText(`low story of ${outside}`)).toHaveCount(0);
  expect(await accessibilityViolations(page)).toEqual([]);

  await page.getByRole('columnheader', { name: 'Key' }).getByRole('button').click();
  await expect(field).toHaveValue(`project IN descendantsOf("${portfolio}") ORDER BY key ASC`);
  await expect(keys).toHaveText([`${first}-1`, `${first}-2`, `${second}-1`, `${second}-2`]);
  await keys.first().click();
  await expect(page).toHaveURL(new RegExp(`/issues/${first}-1$`));
  await page.goBack();
  await expect(field).toHaveValue(`project IN descendantsOf("${portfolio}") ORDER BY key ASC`);

  await field.fill('project = ');
  await field.press('Enter');
  await expect(page.locator('#query-error')).toContainText('Character 10: expected a value, found the end of the query.');
  await expect(field).toBeFocused();
});

test('words with typos find the tickets holding them, those with the words in their title first', async ({ page }) => {
  test.setTimeout(120_000);
  // Unique per run: the stack keeps its data.
  const run = Date.now().toString(36).toUpperCase().slice(-5);
  const project = `FT${run}`;
  await signIn(page, adminEmail!, adminSecret!);
  await page.goto('/');
  await page.evaluate(async (project) => {
    const session = (await (await fetch('/api/v1/auth/session')).json()) as { csrfToken: string };
    const headers = { 'content-type': 'application/json', 'x-csrf-token': session.csrfToken };
    const created = await fetch('/api/v1/nodes', {
      method: 'POST',
      headers,
      body: JSON.stringify({ type: 'project', name: `Project ${project}`, key: project }),
    });
    if (created.status !== 201) throw new Error(`project: ${created.status}`);
    const form = (await (await fetch(`/api/v1/projects/${project}/issue-form`)).json()) as { types: { id: string; key: string }[] };
    const story = form.types.find((type) => type.key === 'story');
    const doc = (text: string) => ({ type: 'doc', content: [{ type: 'paragraph', content: [{ type: 'text', text }] }] });
    // In the description of the first, in the title of the second.
    for (const body of [
      { summary: 'Accounts locked this morning', description: doc('Outage of the directory') },
      { summary: 'Directory outage', description: doc('Nobody can sign in') },
      { summary: 'Printer jam', description: doc('Second floor') },
    ]) {
      const response = await fetch(`/api/v1/projects/${project}/issues`, {
        method: 'POST',
        headers: { ...headers, 'idempotency-key': crypto.randomUUID() },
        body: JSON.stringify({ ...body, typeId: story?.id, priority: 'medium' }),
      });
      if (response.status !== 201) throw new Error(`ticket: ${response.status} ${await response.text()}`);
    }
  }, project);

  await page.getByRole('link', { name: 'Search' }).click();
  await page.getByRole('radio', { name: 'Query' }).click();
  const field = page.getByRole('combobox', { name: 'Query' });
  await field.fill(`project = ${project} AND text ~ "directroy outgae"`);
  await field.press('Enter');
  await expect(page.getByText('Results: 2')).toBeVisible();
  await expect(page.locator('td.key a')).toHaveText([`${project}-2`, `${project}-1`]);
});
