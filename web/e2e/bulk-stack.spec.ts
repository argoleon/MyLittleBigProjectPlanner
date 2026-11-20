/**
 * Bulk actions on the production stack, nothing mocked (L06-US08): the
 * tenant admin ticks two tickets of a search, changes their status within
 * the request, reads the report, and finds them in their new status.
 *
 * Runs with `just compose-e2e` only: deploy/tests/e2e.sh creates the admin
 * and passes E2E_ADMIN_EMAIL and E2E_ADMIN_SECRET.
 */
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

test('two tickets of a search change status together', async ({ page }) => {
  test.setTimeout(120_000);
  // Unique per run: the stack keeps its data.
  const run = Date.now().toString(36).toUpperCase().slice(-5);
  const key = `BK${run}`;
  await signIn(page, adminEmail!, adminSecret!);
  await page.goto('/');
  await page.evaluate(async (key) => {
    const session = (await (await fetch('/api/v1/auth/session')).json()) as { csrfToken: string };
    const headers = { 'content-type': 'application/json', 'x-csrf-token': session.csrfToken };
    const created = await fetch('/api/v1/nodes', {
      method: 'POST',
      headers,
      body: JSON.stringify({ type: 'project', name: `Bulk project ${key}`, key }),
    });
    if (created.status !== 201) throw new Error(`project: ${created.status}`);
    const form = (await (await fetch(`/api/v1/projects/${key}/issue-form`)).json()) as { types: { id: string; key: string }[] };
    const task = form.types.find((type) => type.key === 'task');
    for (const summary of ['First', 'Second']) {
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
  await page.getByRole('checkbox', { name: 'Select the loaded tickets' }).check();
  const bar = page.getByRole('toolbar', { name: 'Actions on the selected tickets' });
  await expect(bar).toContainText('2 selected');
  await bar.getByRole('button', { name: 'Change status' }).click();
  const dialog = page.getByRole('dialog', { name: 'Change the status of 2 tickets' });
  await dialog.getByRole('radio', { name: /In progress/ }).check();
  await dialog.getByRole('button', { name: 'Change the status' }).click();

  const report = page.getByRole('dialog', { name: 'Bulk status change' });
  await expect(report.getByRole('status')).toHaveText('2 changed, 0 already so, 0 left unchanged');
  await expect(report.getByRole('row', { name: new RegExp(`${key}-1`) })).toContainText('Changed');
  await report.getByRole('button', { name: 'Close' }).click();

  // The search, run again, shows them in their new status.
  await expect(page.getByRole('row', { name: new RegExp(`${key}-1 First`) })).toContainText('In progress');
  await expect(page.getByRole('row', { name: new RegExp(`${key}-2 Second`) })).toContainText('In progress');
});
