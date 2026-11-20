import type { Page } from '@playwright/test';

import { accessibilityViolations, expect, test } from './fixtures';

/**
 * Search (L06-US01, L06-US02), with the API mocked: a query typed and run
 * with the keyboard, its results in the virtualized table, a column sorting
 * the query, the next page loaded while scrolling, a refused query shown at
 * its position; the visual builder writing the query, the editor's
 * suggestions with the keyboard alone; a query saved as a filter, opened
 * from the panel, changed and deleted (L06-US04); axe-core in both themes and
 * languages.
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

const value = (text: string, label: string, detail: string | null = null) => ({ kind: 'value', text, label, detail });
const STATUSES = [value('todo', 'To do', 'todo'), value('in_progress', 'In progress', 'in_progress')];
const PROJECTS = [value('PLAT', 'Identity platform', 'project'), value('WEB', 'Website', 'project')];
const FIELDS = ['key', 'project', 'type', 'status', 'priority', 'assignee'];

/** What the mocked API suggests: enough of the language for these tests. */
function suggestions(query: string, cursor: number) {
  const before = query.slice(0, cursor);
  const partial = /[\w.@-]*$/.exec(before)?.[0] ?? '';
  const from = before.length - partial.length;
  const head = before.slice(0, from);
  const starting = (list: { text: string }[]) => list.filter((s) => s.text.toLowerCase().startsWith(partial.toLowerCase()));
  let found: object[] = [];
  if (head.trim() === '' || /\bAND\s+$/i.test(head)) {
    found = starting(FIELDS.map((text) => ({ kind: 'field', text, label: null, detail: null })));
  } else if (/\bstatus\s+$/i.test(head)) {
    found = ['=', '!=', 'IN', 'NOT IN'].map((text) => ({ kind: /\w/.test(text) ? 'keyword' : 'operator', text, label: null, detail: null }));
  } else if (/\bstatus\s*(=|IN \(|IN \([^)]*,)\s*$/i.test(head)) {
    found = starting(STATUSES);
  } else if (/\bproject\s*(=|IN \()\s*$/i.test(head)) {
    found = starting(PROJECTS);
  }
  return { from, to: from + partial.length, suggestions: found };
}

/** The tree of a query made of `field IN (…)` and `field = …` clauses joined by AND. */
function tree(query: string) {
  const filter = query.replace(/\s+ORDER BY .*$/i, '').trim();
  if (!filter) return { query, filter: null, orderBy: [] };
  if (/\sOR\s/i.test(filter)) return { query, filter: { kind: 'or', operands: [] }, orderBy: [] };
  const clauses = filter.split(/\s+AND\s+/i).map((clause) => {
    const [, field, operator, values] = /^(\w+)\s*(IN|=)\s*\(?([^)]*)\)?$/i.exec(clause) ?? [];
    return {
      kind: 'clause',
      field,
      operator: operator?.toUpperCase(),
      values: (values ?? '').split(',').map((v) => ({ kind: 'text', value: v.trim() })),
    };
  });
  return { query, filter: clauses.length === 1 ? clauses[0] : { kind: 'and', operands: clauses }, orderBy: [] };
}

interface MockFilter {
  id: string;
  name: string;
  icon: string;
  query: string;
  owner: { accountId: string; displayName: string; member: boolean };
  sharing: { all: boolean; groups: { id: string; name: string }[] };
  mine: boolean;
  canEdit: boolean;
  canDelete: boolean;
  version: number;
}

const GROUPS = [
  { id: 'g1', name: 'Platform' },
  { id: 'g2', name: 'Payroll' },
];

/** A filter shared with the member by another one. */
const SHARED_WITH_ME: MockFilter = {
  id: 'shared-1',
  name: 'Late',
  icon: 'schedule',
  query: 'due < now()',
  owner: { accountId: 'a2', displayName: 'Camille Martin', member: true },
  sharing: { all: false, groups: [GROUPS[0]] },
  mine: false,
  canEdit: false,
  canDelete: false,
  version: 1,
};

/** The saved filters of the mocked API, in memory: `filters` at first. */
async function mockFilters(page: Page, filters: MockFilter[]) {
  let next = 1;
  const view = (body: { name: string; icon: string; query: string; sharing: { all: boolean; groupIds: string[] } }) => ({
    name: body.name,
    icon: body.icon,
    query: body.query,
    sharing: { all: body.sharing.all, groups: GROUPS.filter((g) => body.sharing.groupIds.includes(g.id)) },
  });
  await page.route(
    (url) => url.pathname.startsWith('/api/v1/saved-filters'),
    async (route) => {
      const request = route.request();
      const path = new URL(request.url()).pathname;
      if (path === '/api/v1/saved-filters/share-groups') {
        const search = new URL(request.url()).searchParams.get('search')?.toLowerCase() ?? '';
        return route.fulfill({ json: GROUPS.filter((g) => g.name.toLowerCase().includes(search)) });
      }
      if (path === '/api/v1/saved-filters') {
        if (request.method() === 'GET') {
          return route.fulfill({ json: { items: [...filters].sort((a, b) => a.name.localeCompare(b.name)) } });
        }
        const created: MockFilter = {
          id: `mine-${next++}`,
          ...view(request.postDataJSON()),
          owner: { accountId: 'a1', displayName: 'Lucas Bernard', member: true },
          mine: true,
          canEdit: true,
          canDelete: true,
          version: 1,
        };
        filters.push(created);
        return route.fulfill({ status: 201, json: created });
      }
      const at = filters.findIndex((f) => path.endsWith(`/${f.id}`));
      if (at < 0) {
        return route.fulfill({
          status: 404,
          contentType: 'application/problem+json',
          body: JSON.stringify({ type: 'urn:mlbpp:problem:not_found', title: 'Not found', status: 404, code: 'not_found' }),
        });
      }
      if (request.method() === 'PUT') {
        filters[at] = { ...filters[at], ...view(request.postDataJSON()), version: filters[at].version + 1 };
      } else if (request.method() === 'DELETE') {
        filters.splice(at, 1);
        return route.fulfill({ status: 204 });
      }
      return route.fulfill({ json: filters[at] });
    },
  );
}

/** The queries the mocked API received, in order; the screen in `mode` (`null`: never chosen yet). */
async function mockSearch(page: Page, mode: 'query' | 'builder' | null = 'query', filters: MockFilter[] = []): Promise<string[]> {
  const queries: string[] = [];
  if (mode) await page.addInitScript((m) => localStorage.setItem('mlbpp.search.mode', m), mode);
  await mockFilters(page, filters);
  await page.route('**/api/v1/search/suggestions', (route) => {
    const body = route.request().postDataJSON() as { query: string; cursor?: number };
    return route.fulfill({ json: suggestions(body.query, body.cursor ?? body.query.length) });
  });
  await page.route('**/api/v1/search/parse', (route) => {
    const body = route.request().postDataJSON() as { query: string };
    return route.fulfill({ json: tree(body.query) });
  });
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

  const field = page.getByRole('combobox', { name: 'Query' });
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
  const field = page.getByRole('combobox', { name: 'Query' });
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

test('the builder writes the query: project PLAT and status In progress', async ({ page }) => {
  const queries = await mockSearch(page, null);
  await page.goto('/search');
  // The builder the first time.
  await expect(page.getByRole('radio', { name: 'Builder' })).toBeChecked();
  const projects = page.getByPlaceholder('Add a project…');
  await projects.fill('PL');
  await page.getByRole('option', { name: 'PLAT · Identity platform' }).click();
  await expect(page).toHaveURL(/q=project%20IN%20\(PLAT\)$/);
  await page.getByRole('combobox', { name: 'Status' }).click();
  await page.getByRole('option', { name: 'In progress' }).click();
  await page.keyboard.press('Escape');
  await expect(page).toHaveURL(/q=project%20IN%20\(PLAT\)%20AND%20status%20IN%20\(in_progress\)$/);

  // The same query, in the editor.
  await page.getByRole('radio', { name: 'Query' }).click();
  await expect(page.getByRole('combobox', { name: 'Query' })).toHaveValue('project IN (PLAT) AND status IN (in_progress)');
  await expect.poll(() => queries.at(-1)).toBe('project IN (PLAT) AND status IN (in_progress)');
});

test('typing stat suggests status, then its keys with their labels, with the keyboard alone', async ({ page }) => {
  const queries = await mockSearch(page);
  await page.goto('/search');
  const editor = page.getByRole('combobox', { name: 'Query' });
  await editor.click();
  await editor.pressSequentially('stat');
  await expect(page.getByRole('option', { name: /^status/ })).toBeVisible();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect(editor).toHaveValue('status ');
  // Its operators, then its values with their labels.
  await expect(page.getByRole('option', { name: /^IN/ })).toBeVisible();
  await editor.pressSequentially('= ');
  await expect(page.getByRole('option').filter({ hasText: 'In progress' })).toBeVisible();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect(editor).toHaveValue('status = in_progress ');
  await page.keyboard.press('Escape');
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(/q=status%20%3D%20in_progress$/);
  await expect.poll(() => queries.at(-1)).toBe('status = in_progress');
});

test('a query the builder can show opens in it with the same criteria; another stays in the editor', async ({ page }) => {
  await mockSearch(page);
  await page.goto('/search?q=status%20IN%20(in_progress)');
  await page.getByRole('radio', { name: 'Builder' }).click();
  await expect(page.getByRole('combobox', { name: 'Status' })).toContainText('In progress');

  // Another query, then the builder asked for: it stays in the editor, with a word why.
  await page.getByRole('radio', { name: 'Query' }).click();
  const editor = page.getByRole('combobox', { name: 'Query' });
  await editor.fill('status = todo OR status = in_progress');
  await editor.press('Escape');
  await editor.press('Enter');
  await expect(page).toHaveURL(/OR%20status/);
  await page.getByRole('radio', { name: 'Builder' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'cannot be shown in the builder' })).toBeVisible();
  await expect(editor).toHaveValue('status = todo OR status = in_progress');
  await expect(page.getByRole('radio', { name: 'Query' })).toBeChecked();
});

test('a query is saved as a filter for the whole organization, opened from the panel, changed, then deleted', async ({ page }) => {
  const queries = await mockSearch(page, 'query', [SHARED_WITH_ME]);
  await page.goto('/search?q=status%20%3D%20in_progress');
  await expect(page.getByText('Results: 70')).toBeVisible();
  const panel = page.getByRole('navigation', { name: 'Saved filters' });
  await expect(panel.getByRole('link', { name: 'Late (by Camille Martin)' })).toBeVisible();
  await expect(panel.getByText('Save a search to find it here.')).toBeVisible();

  await page.getByRole('button', { name: 'Save the filter' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('heading')).toHaveText('Save the filter');
  await dialog.getByLabel('Name').fill('In progress');
  await dialog.getByRole('radio', { name: 'Flag' }).click();
  await dialog.getByRole('radio', { name: 'The whole organization' }).check();
  await dialog.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page).toHaveURL(/\?filter=mine-1$/);
  await expect(page.getByText('Filter “In progress” · shared · Results: 70')).toBeVisible();
  const mine = panel.getByRole('link', { name: 'In progress (shared)' });
  await expect(mine).toHaveAttribute('aria-current', 'page');

  // Somewhere else, then back through the panel: its query runs again.
  await page.goto('/search?q=priority%20%3D%20high');
  await expect(page.getByText('Results: 70')).toBeVisible();
  await mine.click();
  await expect(page).toHaveURL(/\?filter=mine-1$/);
  await expect.poll(() => queries.at(-1)).toBe('status = in_progress');

  // Renamed, then deleted.
  await panel.getByRole('button', { name: 'Actions of the filter In progress' }).click();
  await page.getByRole('menuitem', { name: 'Edit' }).click();
  await dialog.getByLabel('Name').fill('Ongoing');
  await dialog.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(panel.getByRole('link', { name: 'Ongoing (shared)' })).toBeVisible();
  await panel.getByRole('button', { name: 'Actions of the filter Ongoing' }).click();
  await page.getByRole('menuitem', { name: 'Delete' }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Delete' }).click();
  await expect(panel.getByText('Save a search to find it here.')).toBeVisible();
  await expect(page).not.toHaveURL(/filter=/);
  await expect(page.getByRole('status').filter({ hasText: 'Filter deleted.' })).toBeVisible();
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

      test('the saved filters and the dialog saving one have no axe-core violation', async ({ page }) => {
        const mineOne: MockFilter = { ...SHARED_WITH_ME, id: 'mine-0', name: 'Bugs', icon: 'bug_report', mine: true, canEdit: true, canDelete: true };
        await mockSearch(page, 'query', [mineOne, SHARED_WITH_ME]);
        await page.goto('/search?filter=mine-0');
        await expect(page.locator('app-saved-filters a.link[aria-current="page"]')).toBeVisible();
        await expect(page.getByRole('link', { name: 'PLAT-1', exact: true })).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);

        await page.locator('button.save-filter').click();
        const dialog = page.locator('mat-dialog-container');
        await expect(dialog.locator('mat-chip-row')).toHaveCount(1);
        await dialog.locator('input.group-search').fill('pay');
        await expect(page.locator('mat-option')).toHaveCount(1);
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
      });

      test('the builder and the open suggestions have no axe-core violation', async ({ page }) => {
        await mockSearch(page, 'builder');
        await page.goto('/search?q=project%20IN%20(PLAT)');
        await expect(page.locator('app-query-builder mat-chip-row')).toHaveCount(1);
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);

        await page.locator('mat-button-toggle.query-mode button').click();
        const editor = page.locator('app-query-editor textarea');
        await editor.click();
        await editor.press('End');
        await editor.pressSequentially(' AND st');
        await expect(page.locator('mat-option').first()).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
      });
    });
  }
}
