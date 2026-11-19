import type { Page } from '@playwright/test';

import { TENANT, accessibilityViolations, expect, test } from './fixtures';

/**
 * Bulk actions on a search result in the browser, the API mocked
 * (L06-US08): tickets ticked, their status changed, the report shown; all
 * the results of a query changed in the background, followed on its page;
 * its notification leads there; no axe-core violation in either theme or
 * language.
 */

function result(n: number) {
  return {
    id: `0194f0a2-7c1e-7d3a-9b2f-${String(n).padStart(12, '0')}`,
    key: `PLAT-${n}`,
    summary: `Ticket ${n}`,
    project: { id: 'p1', key: 'PLAT', name: 'Identity platform' },
    type: { id: 't1', key: 'task', label: 'Task', labelTranslations: { en: 'Task', fr: 'Tâche' }, icon: 'task_alt', color: 'task', isSubtask: false, parentIds: [] },
    status: { id: 's1', key: 'todo', label: 'To do', category: 'todo' },
    priority: 'high',
    assignee: null,
    reporter: null,
    startDate: null,
    dueDate: null,
    parentId: null,
    createdAt: '2026-11-06T09:12:00Z',
    updatedAt: '2026-11-06T18:20:00Z',
  };
}

/** A bulk status change: done with two tickets, or running over 250. */
function bulkAction(id: string, status: 'done' | 'running') {
  const done = status === 'done';
  return {
    id,
    kind: 'transition',
    status,
    background: !done || id !== 'b1',
    total: done ? 2 : 250,
    counts: { changed: done ? 1 : 0, unchanged: 0, refused: done ? 1 : 0, withParent: 0, waiting: done ? 0 : 250 },
    items: done
      ? [
          { issueId: result(1).id, key: 'PLAT-1', outcome: 'changed', errors: [] },
          { issueId: result(2).id, key: 'PLAT-2', outcome: 'refused', reason: 'transition.restricted', errors: [] },
        ]
      : [],
    createdAt: '2026-11-19T09:00:00Z',
    completedAt: done ? '2026-11-19T09:00:01Z' : null,
    expiresAt: '2026-11-26T09:00:00Z',
  };
}

/** The search screen with 2 of 250 results loaded; bulk actions answered within the request or in the background. */
async function mockBulk(page: Page) {
  const asked: object[] = [];
  let reads = 0;
  await page.addInitScript(() => localStorage.setItem('mlbpp.search.mode', 'query'));
  await page.route('**/api/v1/saved-filters', (route) => route.fulfill({ json: { items: [] } }));
  await page.route('**/api/v1/search/parse', (route) =>
    route.fulfill({ json: { query: (route.request().postDataJSON() as { query: string }).query, filter: null, orderBy: [] } }),
  );
  await page.route('**/api/v1/search', (route) => route.fulfill({ json: { items: [result(1), result(2)], nextCursor: null, total: 250 } }));
  await page.route('**/api/v1/bulk-actions/options', (route) => {
    const body = route.request().postDataJSON() as { query?: string };
    const total = body.query ? 250 : 2;
    return route.fulfill({
      json: {
        total,
        background: total > 100,
        projectKey: 'PLAT',
        fields: [],
        statuses: [
          { key: 'in_progress', label: 'In progress', category: 'in_progress', reachable: total },
          { key: 'done', label: 'Done', category: 'done', reachable: total - 1 },
        ],
      },
    });
  });
  await page.route('**/api/v1/bulk-actions', (route) => {
    const body = route.request().postDataJSON() as { selection: { query?: string } };
    asked.push(body);
    return body.selection.query
      ? route.fulfill({ status: 202, json: bulkAction('b2', 'running') })
      : route.fulfill({ json: bulkAction('b1', 'done') });
  });
  // Running the first time it is read, then done.
  await page.route('**/api/v1/bulk-actions/b2', (route) => {
    reads += 1;
    return route.fulfill({ json: reads > 1 ? bulkAction('b2', 'done') : bulkAction('b2', 'running') });
  });
  return asked;
}

async function animationsDone(page: Page) {
  await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== 'running'));
}

test('ticked tickets change status, the report listing each', async ({ page }) => {
  const asked = await mockBulk(page);
  await page.goto('/search?q=project%20%3D%20PLAT');
  await page.getByRole('checkbox', { name: 'Select PLAT-1' }).check();
  await page.getByRole('checkbox', { name: 'Select PLAT-2' }).check();
  const bar = page.getByRole('toolbar', { name: 'Actions on the selected tickets' });
  await expect(bar).toContainText('2 selected');
  await bar.getByRole('button', { name: 'Change status' }).click();
  const dialog = page.getByRole('dialog', { name: 'Change the status of 2 tickets' });
  await dialog.getByRole('radio', { name: /Done/ }).check();
  await dialog.getByRole('button', { name: 'Change the status' }).click();
  const report = page.getByRole('dialog', { name: 'Bulk status change' });
  await expect(report.getByRole('status')).toHaveText('1 changed, 0 already so, 1 left unchanged');
  await expect(report.getByRole('row', { name: /PLAT-2/ })).toContainText('You may not run the transition');
  expect(asked).toEqual([
    { selection: { issueIds: [result(1).id, result(2).id] }, action: { kind: 'transition', statusKey: 'done' } },
  ]);
});

test('all the results of a query change in the background, followed on its page', async ({ page }) => {
  const asked = await mockBulk(page);
  await page.goto('/search?q=project%20%3D%20PLAT');
  await page.getByRole('checkbox', { name: 'Select the loaded tickets' }).check();
  await page.getByRole('button', { name: 'Select all 250 results' }).click();
  await expect(page.getByRole('toolbar')).toContainText('250 selected');
  await page.getByRole('button', { name: 'Change status' }).click();
  await page.getByRole('dialog').getByRole('radio', { name: /In progress/ }).check();
  await page.getByRole('dialog').getByRole('button', { name: 'Change the status' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'in the background' })).toContainText(
    '250 tickets are being changed in the background: you will be notified when it is done.',
  );
  expect(asked).toEqual([{ selection: { query: 'project = PLAT' }, action: { kind: 'transition', statusKey: 'in_progress' } }]);
  await page.getByRole('link', { name: 'Follow it' }).click();
  await expect(page).toHaveURL(/\/bulk-actions\/b2$/);
  await expect(page.getByText('Still running: this page reads it again.')).toBeVisible();
  // Read again a few seconds later: done.
  await expect(page.getByText('1 changed, 0 already so, 1 left unchanged')).toBeVisible({ timeout: 10_000 });
});

test('the notification of a bulk action done leads to its report', async ({ page }) => {
  await mockBulk(page);
  await page.route('**/api/v1/me/notifications/unread', (route) => route.fulfill({ json: { total: 1, byTenant: [] } }));
  await page.route('**/api/v1/me/notifications?**', (route) =>
    route.fulfill({
      json: {
        items: [
          {
            id: 'n1',
            tenant: { id: TENANT.id, slug: TENANT.slug, name: TENANT.name },
            event: 'bulk_done',
            issueId: null,
            issueKey: null,
            issueSummary: null,
            export: null,
            bulk: { id: 'b2', kind: 'transition', total: 250, changed: 249, unchanged: 0, refused: 1 },
            actorName: null,
            status: null,
            excerpt: null,
            read: false,
            createdAt: new Date().toISOString(),
          },
        ],
        nextCursor: null,
      },
    }),
  );
  await page.route('**/api/v1/me/notifications/read', (route) => route.fulfill({ json: { total: 0, byTenant: [] } }));
  await page.route('**/api/v1/me/notification-preferences', (route) =>
    route.fulfill({ json: { events: [{ event: 'bulk_done', inApp: true, email: true }], dailyDigest: false } }),
  );
  await page.goto('/notifications');
  await page.getByRole('button', { name: /Your bulk status change is done/ }).click();
  await expect(page).toHaveURL(/\/bulk-actions\/b2$/);
});

for (const theme of ['light', 'dark'] as const) {
  for (const language of ['fr', 'en'] as const) {
    test.describe(`accessibility of bulk actions, ${theme} mode, ${language}`, () => {
      test.use({ theme, language });

      test('the selection bar, the status dialog and the report have no axe-core violation', async ({ page }) => {
        await mockBulk(page);
        await page.goto('/search?q=project%20%3D%20PLAT');
        await page.locator('td.select input[type="checkbox"]').first().check();
        await expect(page.locator('.bulk-bar')).toBeVisible();
        expect(await accessibilityViolations(page)).toEqual([]);
        await page.locator('button.bulk-status').click();
        await expect(page.locator('mat-dialog-container mat-radio-button').first()).toBeVisible();
        await page.locator('mat-dialog-container mat-radio-button input').first().check();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
        await page.locator('mat-dialog-container button.confirm').click();
        await expect(page.locator('app-bulk-report')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
      });
    });
  }
}
