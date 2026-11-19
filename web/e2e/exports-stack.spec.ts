/**
 * Exports on the production stack, nothing mocked (L06-US06): the tenant
 * admin exports a search to CSV and the browser downloads it from the
 * storage through the reverse proxy: UTF-8 with a byte order mark, the
 * columns displayed with their headers, a text starting like a formula
 * neutralized.
 *
 * Runs with `just compose-e2e` only: deploy/tests/e2e.sh creates the admin
 * and passes E2E_ADMIN_EMAIL and E2E_ADMIN_SECRET.
 */
import { readFile } from 'node:fs/promises';

import { type Page, expect, test } from '@playwright/test';

const adminEmail = process.env['E2E_ADMIN_EMAIL'];
const adminSecret = process.env['E2E_ADMIN_SECRET'];

test.skip(!adminEmail || !adminSecret, 'needs the compose stack (just compose-e2e)');

async function signIn(page: Page, email: string, secret: string) {
  await page.context().addInitScript(() => {
    localStorage.setItem('mlbpp.language', 'en');
    localStorage.setItem('mlbpp.search.mode', 'query');
    localStorage.setItem('mlbpp.search.columns', JSON.stringify(['key', 'summary', 'status']));
  });
  await page.goto('/sign-in');
  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password').fill(secret);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');
}

test('a search is exported to CSV and downloaded from the storage', async ({ page }) => {
  test.setTimeout(120_000);
  // Unique per run: the stack keeps its data.
  const run = Date.now().toString(36).toUpperCase().slice(-5);
  const key = `EX${run}`;
  await signIn(page, adminEmail!, adminSecret!);
  await page.goto('/');
  await page.evaluate(async (key) => {
    const session = (await (await fetch('/api/v1/auth/session')).json()) as { csrfToken: string };
    const headers = { 'content-type': 'application/json', 'x-csrf-token': session.csrfToken };
    const created = await fetch('/api/v1/nodes', {
      method: 'POST',
      headers,
      body: JSON.stringify({ type: 'project', name: `Export project ${key}`, key }),
    });
    if (created.status !== 201) throw new Error(`project: ${created.status}`);
    const form = (await (await fetch(`/api/v1/projects/${key}/issue-form`)).json()) as { types: { id: string; key: string }[] };
    const task = form.types.find((type) => type.key === 'task');
    for (const summary of ['Créer l’annuaire', '=1+1, then "more"']) {
      const response = await fetch(`/api/v1/projects/${key}/issues`, {
        method: 'POST',
        headers: { ...headers, 'idempotency-key': crypto.randomUUID() },
        body: JSON.stringify({ summary, typeId: task?.id, priority: 'medium' }),
      });
      if (response.status !== 201) throw new Error(`ticket: ${response.status}`);
    }
  }, key);

  await page.goto(`/search?q=${encodeURIComponent(`project = ${key} ORDER BY key ASC`)}`);
  await expect(page.getByText('Results: 2')).toBeVisible();
  await page.getByRole('button', { name: 'Export' }).click();
  const downloaded = page.waitForEvent('download');
  await page.getByRole('menuitem', { name: 'CSV file (.csv)' }).click();
  const download = await downloaded;
  expect(download.suggestedFilename()).toMatch(/^search-\d{4}-\d{2}-\d{2}\.csv$/);
  const bytes = await readFile((await download.path())!);
  expect([...bytes.subarray(0, 3)]).toEqual([0xef, 0xbb, 0xbf]);
  expect(bytes.toString('utf8')).toBe(
    `\u{feff}Key,Title,Status\r\n${key}-1,Créer l’annuaire,To do\r\n${key}-2,"'=1+1, then ""more""",To do\r\n`,
  );
});
