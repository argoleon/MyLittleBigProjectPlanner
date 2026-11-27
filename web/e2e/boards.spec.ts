import type { Locator, Page } from '@playwright/test';

import { accessibilityViolations, expect, problem, test } from './fixtures';

/**
 * The Kanban board in the browser, the API mocked (L06-US05): a card
 * dragged to the next column moves at once and runs the transition; one
 * dragged where no transition leads goes back with the reason; another
 * moved from its menu with the keyboard alone; a column over its limit;
 * no axe-core violation in either theme or language.
 */

const STATUS = {
  todo: { id: 's1', key: 'todo', label: 'To do', category: 'todo' },
  in_progress: { id: 's2', key: 'in_progress', label: 'In progress', category: 'in_progress' },
  in_review: { id: 's3', key: 'in_review', label: 'In review', category: 'in_progress' },
  done: { id: 's4', key: 'done', label: 'Done', category: 'done' },
} as const;

type StatusKey = keyof typeof STATUS;

const COLUMNS = [
  { id: 'c-todo', name: 'To do', statuses: ['todo'], wipLimit: null },
  { id: 'c-progress', name: 'In progress', statuses: ['in_progress'], wipLimit: 2 },
  { id: 'c-review', name: 'Review', statuses: ['in_review'], wipLimit: null },
  { id: 'c-done', name: 'Done', statuses: ['done'], wipLimit: null },
];

const BOARD = {
  id: 'b1',
  node: { id: 'p1', key: 'PLAT', name: 'Identity platform', nodeType: 'project', archived: false },
  name: 'Platform board',
  kind: 'kanban',
  filter: 'project = PLAT AND status != cancelled',
  estimation: 'points',
  swimlanes: 'none',
  quickFilters: [
    { label: 'My tickets', query: 'assignee = currentUser()' },
    { label: 'Bugs', query: 'type = bug' },
  ],
  columns: COLUMNS,
  canManage: true,
  canPlan: false,
  version: 1,
};

interface Ticket {
  n: number;
  status: StatusKey;
  version: number;
}

function card(t: Ticket) {
  return {
    id: `i${t.n}`,
    key: `PLAT-${t.n}`,
    summary: `Ticket ${t.n}`,
    projectId: 'p1',
    type: { id: 't1', key: 'task', label: 'Task', labelTranslations: { en: 'Task', fr: 'Tâche' }, icon: 'task_alt', color: 'task', isSubtask: false, parentIds: [] },
    status: STATUS[t.status],
    priority: 'medium',
    assignee: t.n % 2 ? { id: 'a2', name: 'Camille Martin' } : null,
    estimate: t.n,
    epic: null,
    blockedBy: t.n === 4 ? ['PLAT-1'] : [],
    version: t.version,
  };
}

/** The workflow of the mock: To do → In progress → In review → Done, nothing back to To do. */
const NEXT: Readonly<Record<StatusKey, readonly StatusKey[]>> = {
  todo: ['in_progress'],
  in_progress: ['in_review'],
  in_review: ['in_progress', 'done'],
  done: [],
};

async function mockBoard(page: Page) {
  const tickets: Ticket[] = [
    { n: 1, status: 'todo', version: 1 },
    { n: 2, status: 'todo', version: 1 },
    { n: 3, status: 'in_progress', version: 1 },
    { n: 4, status: 'in_progress', version: 1 },
    { n: 5, status: 'in_review', version: 1 },
  ];
  const moves: object[] = [];
  await page.route('**/api/v1/boards/b1', (route) => route.fulfill({ json: BOARD }));
  await page.route(
    (url) => url.pathname === '/api/v1/boards/b1/cards',
    (route) => {
      const columns = COLUMNS.map((column) => {
        const count = tickets.filter((t) => column.statuses.includes(t.status)).length;
        const wip = column.wipLimit === null ? 'unlimited' : count > column.wipLimit ? 'exceeded' : count === column.wipLimit ? 'reached' : 'under';
        return { ...column, count, wip };
      });
      const cells = COLUMNS.map((column) => tickets.filter((t) => column.statuses.includes(t.status)).map(card));
      return route.fulfill({
        json: { columns, swimlanes: 'none', lanes: [{ id: null, key: null, label: null, cells }], total: tickets.length, unmapped: 0, truncated: false },
      });
    },
  );
  await page.route('**/api/v1/boards/b1/moves', (route) => {
    const body = route.request().postDataJSON() as { issueId: string; columnId: string };
    moves.push(body);
    const ticket = tickets.find((t) => `i${t.n}` === body.issueId)!;
    const to = COLUMNS.find((c) => c.id === body.columnId)!.statuses[0] as StatusKey;
    if (!NEXT[ticket.status].includes(to)) return route.fulfill(problem(409, 'board.no_transition'));
    ticket.status = to;
    ticket.version += 1;
    return route.fulfill({
      json: { outcome: 'moved', issue: { id: body.issueId, key: `PLAT-${ticket.n}`, status: STATUS[to], version: ticket.version }, transitions: [] },
    });
  });
  return { moves, tickets };
}

function column(page: Page, name: string): Locator {
  const index = COLUMNS.findIndex((c) => c.name === name);
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

async function animationsDone(page: Page) {
  await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== 'running'));
}

test('a card dragged to the next column runs its transition; one dragged where none leads goes back', async ({ page }) => {
  const { moves } = await mockBoard(page);
  await page.goto('/boards/b1');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Platform board');
  await expect(page.locator('.column-head').nth(1)).toHaveAttribute('data-wip', 'reached');

  await drag(page, cardOf(page, 'PLAT-1'), column(page, 'In progress'));
  await expect(column(page, 'In progress').locator('.key')).toHaveText(['PLAT-1', 'PLAT-3', 'PLAT-4'].sort());
  await expect(page.locator('.column-head').nth(1)).toHaveAttribute('data-wip', 'exceeded');
  await expect(page.locator('.column-head').nth(1).locator('.wip')).toHaveText('Limit exceeded · 3 / 2');

  await drag(page, cardOf(page, 'PLAT-3'), column(page, 'To do'));
  await expect(page.getByRole('alert')).toContainText('PLAT-3 went back: no transition of its workflow leads from In progress to To do.');
  await expect(column(page, 'In progress').locator('.key', { hasText: 'PLAT-3' })).toBeVisible();
  await expect(column(page, 'To do').locator('.key')).toHaveText(['PLAT-2']);
  expect(moves).toEqual([
    { issueId: 'i1', columnId: 'c-progress' },
    { issueId: 'i3', columnId: 'c-todo' },
  ]);
});

test('a card is moved to another column with the keyboard alone', async ({ page }) => {
  const { moves } = await mockBoard(page);
  await page.goto('/boards/b1');
  const menu = page.getByRole('button', { name: 'Move PLAT-5' });
  await menu.focus();
  await page.keyboard.press('Enter');
  // The other columns, the first one focused.
  await expect(page.getByRole('menuitem')).toHaveText(['Move to To do', 'Move to In progress', 'Move to Done']);
  await expect(page.getByRole('menuitem', { name: 'Move to To do' })).toBeFocused();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('ArrowDown');
  await expect(page.getByRole('menuitem', { name: 'Move to Done' })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(column(page, 'Done').locator('.key')).toHaveText(['PLAT-5']);
  expect(moves).toEqual([{ issueId: 'i5', columnId: 'c-done' }]);
});

for (const theme of ['light', 'dark'] as const) {
  for (const language of ['fr', 'en'] as const) {
    test.describe(`accessibility of the board, ${theme} mode, ${language}`, () => {
      test.use({ theme, language });

      test('the board, a refusal and the move menu have no axe-core violation', async ({ page }) => {
        await mockBoard(page);
        await page.goto('/boards/b1');
        await expect(page.locator('app-board-card')).toHaveCount(5);
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);

        await cardOf(page, 'PLAT-3').locator('button.move').click();
        await page.locator('.move-to').first().click();
        await expect(page.getByRole('alert')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);

        await cardOf(page, 'PLAT-2').locator('button.move').click();
        await expect(page.locator('.move-to').first()).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
      });
    });
  }
}
