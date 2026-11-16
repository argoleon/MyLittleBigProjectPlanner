import type { Page } from '@playwright/test';

import { accessibilityViolations, expect, test } from './fixtures';

/**
 * Search (L06-US01), with the API mocked: a query typed and run with the
 * keyboard, its results in the virtualized table, a column sorting the
 * query, the next page loaded while scrolling, a refused query shown at its
 * position; axe-core in both themes and languages.
 */

function result(n: number, priority = 'high') {
  return {
    id: `0194f0a2-7c1e-7d3a-9b2f-${String(n).padStart(12, '0')}`,
    key: `PLAT-${n}`,
    summary: `Ticket ${n}`,
    project: { id: 'p1', key: 'PLAT', name: 'Identity platform' },
    type: { id: 't1', key: 'task', label: 'Task', labelTranslations: { en: 'Task', fr: 'Tâche' }, icon: 'task_alt', color: 'task', isSubtask: false, parentIds: [] },
    status: { id: 's1', key: 'in_progress', label: 'In progress', category: 'in_progress' },
    priority,
    assignee: { id: 'a1', name: 'Lucas Bernard' },
    reporter: { id: 'a2', name: 'Camille Martin' },
    startDate: null,
    dueDate: '2026-11-20',
    parentId: null,
    createdAt: '2026-11-06T09:12:00Z',
    updatedAt: '2026-11-06T18:20:00Z',
  };
}

/** The queries the mocked API received, in order. */
async function mockSearch(page: Page): Promise<string[]> {
  const queries: string[] = [];
  await page.route('**/api/v1/search', (route) => {
    const body = route.request().postDataJSON() as { query: string; cursor?: string };
    queries.push(body.cursor ? `${body.query} @${body.cursor}` : body.query);
    if (body.query.startsWith('status =') && !body.query.includes('in_progress')) {
      return route.fulfill({
        status: 400,
        contentType: 'application/problem+json',
        body: JSON.stringify({
          type: 'urn:mlbpp:problem:query.invalid',
          title: 'The query cannot be run',
          status: 400,
          code: 'query.invalid',
          query: { reason: 'syntax', position: 8, length: 0, expected: ['a value'], found: null },
        }),
      });
    }
    if (body.cursor === 'page-2') {
      return route.fulfill({ json: { items: Array.from({ length: 20 }, (_, i) => result(51 + i, 'low')), nextCursor: null, total: null } });
    }
    return route.fulfill({
      json: { items: Array.from({ length: 50 }, (_, i) => result(i + 1)), nextCursor: 'page-2', total: 70 },
    });
  });
  return queries;
}

async function animationsDone(page: Page) {
  await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== 'running'));
}

test('a query is run with the keyboard, sorted from a column, and its next page loaded while scrolling', async ({ page }) => {
  const queries = await mockSearch(page);
  await page.goto('/search');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Search');
  await expect(page.getByText('Results: 70')).toBeVisible();

  const field = page.getByRole('textbox', { name: 'Query' });
  await field.fill('status = in_progress');
  await field.press('Enter');
  await expect(page).toHaveURL(/\/search\?q=status%20%3D%20in_progress$/);
  await expect(page.getByRole('link', { name: 'PLAT-1', exact: true })).toBeVisible();
  // Only the rows in view are drawn.
  expect(await page.locator('tr.mat-mdc-row').count()).toBeLessThan(50);

  await page.getByRole('columnheader', { name: 'Priority' }).getByRole('button').click();
  await expect(page).toHaveURL(/q=status%20%3D%20in_progress%20ORDER%20BY%20priority%20ASC$/);
  await expect(field).toHaveValue('status = in_progress ORDER BY priority ASC');
  await expect(page.getByRole('columnheader', { name: 'Priority' })).toHaveAttribute('aria-sort', 'ascending');

  // Down the list: the next page comes through the cursor, and the last row
  // can be scrolled whole into view below the sticky header.
  const viewport = page.locator('cdk-virtual-scroll-viewport');
  const last = page.locator('tr.mat-mdc-row', { has: page.getByRole('link', { name: 'PLAT-70', exact: true }) });
  await expect(async () => {
    // Scrolled, then a moment for the table to draw its new range.
    await viewport.evaluate((element) => element.scrollTo({ top: element.scrollHeight }));
    await page.waitForTimeout(100);
    // Not drawn yet: no box (boundingBox() would wait for it).
    const row = (await last.count()) > 0 ? await last.boundingBox() : null;
    const box = await viewport.boundingBox();
    expect(row && box && row.y + row.height <= box.y + box.height + 1).toBe(true);
  }).toPass({ timeout: 10_000 });
  expect(queries).toEqual([
    '',
    'status = in_progress',
    'status = in_progress ORDER BY priority ASC',
    'status = in_progress ORDER BY priority ASC @page-2',
  ]);
});

test('a refused query says where, the part concerned selected in the field', async ({ page }) => {
  await mockSearch(page);
  await page.goto('/search?q=status%20%3D');
  const alert = page.locator('#query-error');
  await expect(alert).toHaveText(/Character 9: expected a value, found the end of the query\./);
  const field = page.getByRole('textbox', { name: 'Query' });
  await expect(field).toBeFocused();
  await expect(field).toHaveAttribute('aria-invalid', 'true');
  expect(await field.evaluate((textarea: HTMLTextAreaElement) => textarea.selectionStart)).toBe(8);
});

test('the columns are chosen from a menu and kept for the next visit', async ({ page }) => {
  await mockSearch(page);
  await page.goto('/search');
  await page.getByRole('button', { name: 'Columns' }).click();
  await page.getByRole('menuitemcheckbox', { name: 'Reporter' }).click();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('columnheader', { name: 'Reporter' })).toBeVisible();
  await page.reload();
  await expect(page.getByRole('columnheader', { name: 'Reporter' })).toBeVisible();
});

for (const theme of ['light', 'dark'] as const) {
  for (const language of ['fr', 'en'] as const) {
    test.describe(`accessibility of search, ${theme} mode, ${language}`, () => {
      test.use({ theme, language });

      test('the results, the columns menu and a refused query have no axe-core violation', async ({ page }) => {
        await mockSearch(page);
        await page.goto('/search?q=status%20%3D%20in_progress');
        await expect(page.getByRole('link', { name: 'PLAT-1', exact: true })).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);

        await page.locator('button.columns').click();
        await expect(page.locator('button.column-choice').first()).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
        await page.keyboard.press('Escape');

        await page.goto('/search?q=status%20%3D');
        await expect(page.locator('#query-error')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
      });
    });
  }
}
