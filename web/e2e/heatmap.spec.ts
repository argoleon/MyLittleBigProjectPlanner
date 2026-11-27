/**
 * The workload heatmap with the API mocked (L08-US06): the grid drawn in
 * the theme's colors, a cell opened with the keyboard and from the table
 * view, the filters reachable; axe-core in light and dark modes, in French
 * and English.
 */
import type { Page } from '@playwright/test';

import { accessibilityViolations, expect, test } from './fixtures';

const PERIODS = Array.from({ length: 8 }, (_, i) => {
  const start = new Date(Date.UTC(2026, 10, 16 + i * 7));
  const end = new Date(Date.UTC(2026, 10, 22 + i * 7));
  return { start: start.toISOString().slice(0, 10), end: end.toISOString().slice(0, 10) };
});

function row(id: string, displayName: string, load: number[], extra: Partial<Record<'provisional' | 'hidden' | 'capacity' | 'scheduled', number[]>> = {}) {
  return {
    resource: { id, kind: 'person', displayName, jobTitle: 'Dev back', linked: true },
    load,
    provisional: extra.provisional ?? load.map(() => 0),
    hidden: extra.hidden ?? load.map(() => 0),
    capacity: extra.capacity ?? load.map(() => 2100),
    scheduled: extra.scheduled ?? load.map(() => 2100),
  };
}

const HEATMAP = {
  scale: 'week',
  periods: PERIODS,
  rows: [
    row('r1', 'Camille Martin', [1722, 1974, 2352, 2100, 1848, 1596, 1260, 840]),
    row('r2', 'Inès Moreau', [2100, 2016, 1848, 1512, 0, 0, 1344, 0], { capacity: [2100, 2100, 2100, 2100, 0, 0, 2100, 2100] }),
    row('r3', 'Sarah Lefèvre', [1638, 1806, 1890, 1344, 1470, 1218, 840, 420], { hidden: [1638, 1806, 1890, 0, 0, 0, 0, 0] }),
    row('r4', 'Léa Fontaine', [1260, 1512, 1218, 1386, 1050, 945, 420, 210], { provisional: [1260, 1512, 0, 0, 0, 945, 0, 0] }),
  ],
  tolerancePercent: 0,
  includeProvisional: true,
};

const CELL = {
  resource: { id: 'r1', kind: 'person', displayName: 'Camille Martin', linked: true },
  from: PERIODS[0].start,
  to: PERIODS[0].end,
  capacityMinutes: 2100,
  scheduledMinutes: 2100,
  loadMinutes: 1722,
  provisionalMinutes: 0,
  items: [
    {
      issue: { id: 'i1', key: 'PLAT-142', summary: 'OIDC sign-in', project: { id: 'p1', key: 'PLAT', name: 'Identity platform' } },
      minutes: 1050,
      provisional: false,
      segments: [{ mode: 'percentage', value: 50 }],
      inNode: false,
    },
    { issue: null, minutes: 672, provisional: false, segments: [], inNode: false },
  ],
  forced: [],
};

async function mockHeatmap(page: Page) {
  await page.route('**/api/v1/teams', (route) =>
    route.fulfill({ json: { items: [{ id: 't1', name: 'Platform', nameTranslations: { en: 'Platform', fr: 'Plateforme' }, memberCount: 4, version: 1 }], canManage: false } }),
  );
  await page.route('**/api/v1/nodes?*', (route) =>
    route.fulfill({ json: { items: [{ id: 'n1', type: 'program', name: 'Modernization', key: 'MOD', parents: [], archived: false }], nextCursor: null } }),
  );
  await page.route('**/api/v1/workload/heatmap?*', (route) => route.fulfill({ json: HEATMAP }));
  await page.route('**/api/v1/workload/cell?*', (route) => route.fulfill({ json: CELL }));
}

/** The color drawn at a point of the canvas, as `r,g,b`. */
async function pixel(page: Page, x: number, y: number): Promise<string> {
  return page.locator('ui-heatmap canvas').evaluate(
    (canvas: HTMLCanvasElement, [px, py]) => {
      const ratio = window.devicePixelRatio || 1;
      const data = canvas.getContext('2d')!.getImageData(px * ratio, py * ratio, 1, 1).data;
      return `${data[0]},${data[1]},${data[2]}`;
    },
    [x, y],
  );
}

test('the heatmap is drawn in the theme colors, its cells open by keyboard and from the table', async ({ page }) => {
  await mockHeatmap(page);
  await page.goto('/workload');
  await expect(page).toHaveURL(/\/workload\/heatmap$/);
  await expect(page.locator('.summary')).toContainText('Resources: 4');
  // Drawn: a surface background, not the canvas' default black.
  await expect.poll(() => pixel(page, 4, 4)).not.toBe('0,0,0');
  // The first cell has the color of its level, unlike the background.
  await expect.poll(async () => (await pixel(page, 240 + 36, 64 + 28)) !== (await pixel(page, 4, 4))).toBe(true);

  // The filters stay reachable over the grid.
  // Its label, not floated while empty, lies over the select: click the field.
  await page.locator('mat-form-field', { has: page.getByRole('combobox', { name: 'Portfolio, program or project' }) }).click();
  await expect(page.getByRole('option', { name: 'Modernization' })).toBeVisible();
  await page.keyboard.press('Escape');

  await page.getByRole('application', { name: 'Resource workload' }).focus();
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('Enter');
  const panel = page.locator('aside.panel');
  await expect(panel.locator('h2')).toHaveText('Camille Martin');
  await expect(panel.locator('.busy')).toHaveText('Busy (confidential)');
  await page.getByRole('button', { name: 'Close the details' }).click();

  await page.locator('mat-button-toggle', { hasText: 'Table' }).locator('button').click();
  const overloaded = page.locator('tr.mat-mdc-row', { hasText: 'Camille Martin' }).locator('button.cell-button', { hasText: 'overloaded' });
  await expect(overloaded).toHaveCount(1);
  await overloaded.click();
  await expect(panel.locator('h2')).toHaveText('Camille Martin');
});

for (const theme of ['light', 'dark'] as const) {
  for (const language of ['fr', 'en'] as const) {
    test.describe(`accessibility of the heatmap, ${theme} mode, ${language}`, () => {
      test.use({ theme, language });

      test('the heatmap, a cell opened and the table view have no axe-core violation', async ({ page }) => {
        await mockHeatmap(page);
        await page.goto('/workload/heatmap');
        await expect(page.locator('ui-heatmap')).toBeVisible();
        await expect.poll(() => pixel(page, 4, 4)).not.toBe('0,0,0');
        expect(await accessibilityViolations(page)).toEqual([]);
        await page.getByRole('application').focus();
        await page.keyboard.press('Enter');
        await expect(page.locator('aside.panel h2')).toBeVisible();
        expect(await accessibilityViolations(page)).toEqual([]);
        await page.locator('mat-button-toggle').nth(4).locator('button').click();
        await expect(page.locator('tr.mat-mdc-row')).toHaveCount(5);
        expect(await accessibilityViolations(page)).toEqual([]);
      });
    });
  }
}
