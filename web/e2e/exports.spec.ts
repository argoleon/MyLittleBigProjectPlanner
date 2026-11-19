import type { Page } from '@playwright/test';

import { TENANT, accessibilityViolations, expect, test } from './fixtures';

/**
 * Exports of list views in the browser, the API mocked (L06-US06): a small
 * search exported downloads its CSV at once; a large one is announced,
 * followed on its page until ready; its notification leads there; no
 * axe-core violation in either theme or language.
 */

const CSV = '\u{feff}Key,Summary\r\nPLAT-1,Ticket 1\r\n';

function exported(id: string, status: 'queued' | 'ready') {
  return {
    id,
    fileName: 'search-2026-11-18.csv',
    format: 'csv',
    status,
    rows: status === 'ready' ? 20001 : null,
    sizeBytes: status === 'ready' ? 812000 : null,
    failure: null,
    downloadPath: status === 'ready' ? `/api/v1/exports/${id}/file` : null,
    createdAt: '2026-11-18T09:00:00Z',
    completedAt: status === 'ready' ? '2026-11-18T09:01:00Z' : null,
    expiresAt: '2026-11-19T09:00:00Z',
  };
}

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

/** The search screen with one result, and exports answered `ready` or `queued`; the bodies asked, kept. */
async function mockExports(page: Page, answer: 'queued' | 'ready') {
  const asked: object[] = [];
  let reads = 0;
  await page.addInitScript(() => localStorage.setItem('mlbpp.search.mode', 'query'));
  await page.route('**/api/v1/saved-filters', (route) => route.fulfill({ json: { items: [] } }));
  await page.route('**/api/v1/search/parse', (route) =>
    route.fulfill({ json: { query: (route.request().postDataJSON() as { query: string }).query, filter: null, orderBy: [] } }),
  );
  await page.route('**/api/v1/search', (route) => route.fulfill({ json: { items: [result(1)], nextCursor: null, total: 1 } }));
  await page.route('**/api/v1/exports', (route) => {
    asked.push(route.request().postDataJSON() as object);
    const id = answer === 'ready' ? 'e1' : 'e2';
    return route.fulfill({ status: answer === 'ready' ? 201 : 202, json: exported(id, answer) });
  });
  await page.route('**/api/v1/exports/e1/file', (route) =>
    route.fulfill({
      status: 200,
      headers: { 'content-type': 'text/csv; charset=utf-8', 'content-disposition': 'attachment; filename="search-2026-11-18.csv"' },
      body: CSV,
    }),
  );
  // Queued the first time it is read, then ready.
  await page.route('**/api/v1/exports/e2', (route) => {
    reads += 1;
    return route.fulfill({ json: exported('e2', reads > 1 ? 'ready' : 'queued') });
  });
  return asked;
}

async function animationsDone(page: Page) {
  await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== 'running'));
}

test('a small search is exported to CSV at once, with its columns', async ({ page }) => {
  const asked = await mockExports(page, 'ready');
  await page.goto('/search?q=project%20%3D%20PLAT');
  await expect(page.getByRole('link', { name: 'PLAT-1', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Export' }).click();
  const downloaded = page.waitForEvent('download');
  await page.getByRole('menuitem', { name: 'CSV file (.csv)' }).click();
  const download = await downloaded;
  expect(download.suggestedFilename()).toBe('search-2026-11-18.csv');
  expect(asked).toEqual([
    {
      query: 'project = PLAT',
      columns: ['key', 'summary', 'type', 'status', 'assignee', 'priority', 'due'],
      format: 'csv',
      name: 'search',
    },
  ]);
});

test('a small search is exported to Excel at once', async ({ page }) => {
  const asked = await mockExports(page, 'ready');
  await page.route('**/api/v1/exports/e1/file', (route) =>
    route.fulfill({
      status: 200,
      headers: {
        'content-type': 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
        'content-disposition': 'attachment; filename="search-2026-11-18.xlsx"',
      },
      body: Buffer.from('PK'),
    }),
  );
  await page.goto('/search?q=project%20%3D%20PLAT');
  await expect(page.getByRole('link', { name: 'PLAT-1', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Export' }).click();
  const downloaded = page.waitForEvent('download');
  await page.getByRole('menuitem', { name: 'Excel file (.xlsx)' }).click();
  expect((await downloaded).suggestedFilename()).toBe('search-2026-11-18.xlsx');
  expect(asked).toEqual([expect.objectContaining({ format: 'xlsx', name: 'search' })]);
});

test('a large export is announced, then followed on its page until it downloads', async ({ page }) => {
  await mockExports(page, 'queued');
  await page.goto('/search?q=project%20%3D%20PLAT');
  await page.getByRole('button', { name: 'Export' }).click();
  await page.getByRole('menuitem', { name: 'CSV file (.csv)' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'is being prepared' })).toContainText(
    'search-2026-11-18.csv is being prepared: you will be notified when it is ready.',
  );
  await page.getByRole('link', { name: 'Follow it' }).click();
  await expect(page).toHaveURL(/\/exports\/e2$/);
  await expect(page.getByText('Being prepared: you will be notified when it is ready.')).toBeVisible();
  // Read again a few seconds later: ready.
  await expect(page.getByRole('link', { name: 'Download' })).toHaveAttribute('href', '/api/v1/exports/e2/file', { timeout: 10_000 });
  await expect(page.getByText('Ready: 20001 rows.')).toBeVisible();
});

test('the notification of an export ready leads to its page', async ({ page }) => {
  await mockExports(page, 'queued');
  await page.route('**/api/v1/me/notifications/unread', (route) => route.fulfill({ json: { total: 1, byTenant: [] } }));
  await page.route('**/api/v1/me/notifications?**', (route) =>
    route.fulfill({
      json: {
        items: [
          {
            id: 'n1',
            tenant: { id: TENANT.id, slug: TENANT.slug, name: TENANT.name },
            event: 'export_ready',
            issueId: null,
            issueKey: null,
            issueSummary: null,
            export: { id: 'e2', fileName: 'search-2026-11-18.csv', rows: 20001 },
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
    route.fulfill({ json: { events: [{ event: 'export_ready', inApp: true, email: false }], dailyDigest: false } }),
  );
  await page.goto('/notifications');
  await page.getByRole('button', { name: /Your export search-2026-11-18\.csv is ready/ }).click();
  await expect(page).toHaveURL(/\/exports\/e2$/);
});

for (const theme of ['light', 'dark'] as const) {
  for (const language of ['fr', 'en'] as const) {
    test.describe(`accessibility of exports, ${theme} mode, ${language}`, () => {
      test.use({ theme, language });

      test('the export menu, its announcement and the export page have no axe-core violation', async ({ page }) => {
        await mockExports(page, 'queued');
        await page.goto('/search?q=project%20%3D%20PLAT');
        await page.locator('button.export').click();
        await expect(page.locator('button.csv')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
        await page.locator('button.csv').click();
        await expect(page.locator('app-export-menu [role="status"]')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);

        // Followed from there, in the application (no reload).
        await page.locator('app-export-menu [role="status"] a').click();
        await expect(page.locator('app-export-page [role="status"]')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
        await expect(page.locator('a.download')).toBeVisible({ timeout: 10_000 });
        expect(await accessibilityViolations(page)).toEqual([]);
      });
    });
  }
}
