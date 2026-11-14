import type { Page } from '@playwright/test';

import { accessibilityViolations, expect, problem, test } from './fixtures';

/**
 * Tickets (L05-US01), with the API mocked: a member creates a ticket from
 * "My tickets" with a rich description and a required field, then lands on
 * its page; axe-core on every screen, in both themes and languages.
 */

const PROJECT = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7c03',
  type: 'project',
  name: 'Identity platform',
  key: 'PLAT',
  description: '',
  objectives: '',
  startDate: null,
  endDate: null,
  health: 'on_track',
  archived: false,
  confidential: false,
  owners: [],
  parents: [],
  version: 1,
};

const TYPE = (id: string, key: string, en: string, color: string, icon: string, parentIds: string[], isSubtask = false) => ({
  id,
  key,
  label: en,
  labelTranslations: { en },
  icon,
  color,
  isSubtask,
  parentIds,
});

const TYPES = [
  TYPE('t-epic', 'epic', 'Epic', 'epic', 'bolt', []),
  TYPE('t-story', 'story', 'Story', 'story', 'bookmark', ['t-epic']),
  TYPE('t-bug', 'bug', 'Bug', 'bug', 'bug_report', ['t-epic']),
  TYPE('t-sub', 'subtask', 'Sub-task', 'task', 'subdirectory_arrow_right', ['t-story', 't-bug'], true),
];

const FORM = {
  project: { id: PROJECT.id, key: 'PLAT', name: PROJECT.name },
  types: TYPES,
  typeId: 't-story',
  fields: [
    {
      fieldId: 'f-app',
      key: 'application',
      label: 'Application',
      kind: 'single_select',
      mode: 'required',
      values: [
        { id: 'v-dir', label: 'Directory', children: [] },
        { id: 'v-mail', label: 'Mail', children: [] },
      ],
    },
    { fieldId: 'f-labels', key: 'labels', label: 'Labels', kind: 'labels', mode: 'visible' },
    { fieldId: 'f-points', key: 'points', label: 'Estimate', kind: 'number', mode: 'visible', decimals: 0 },
  ],
};

const ISSUE = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7e01',
  key: 'PLAT-142',
  project: FORM.project,
  type: TYPES[1],
  status: { id: 's1', key: 'todo', label: 'To do', category: 'todo' },
  summary: 'Migrate the authentication to OIDC',
  description: null,
  descriptionHtml:
    '<p>Replace the <strong>local</strong> accounts by <a href="https://example.org/oidc" rel="noopener noreferrer nofollow" target="_blank">OIDC</a>.</p><ul><li><p>Entra ID</p></li><li><p>Fallback for contractors</p></li></ul>',
  priority: 'high',
  reporter: { id: 'a1', name: 'Lucas Bernard' },
  assignee: { id: 'a2', name: 'Camille Martin' },
  startDate: '2026-11-09',
  dueDate: '2026-12-04',
  fields: [
    { fieldId: 'f-app', key: 'application', label: 'Application', kind: 'single_select', value: 'v-dir', display: 'Directory' },
    { fieldId: 'f-points', key: 'points', label: 'Estimate', kind: 'number', value: 8, display: null },
  ],
  parentId: null,
  createdAt: '2026-11-06T09:12:00Z',
  updatedAt: '2026-11-06T18:20:00Z',
  version: 1,
};

const SUMMARY = {
  id: ISSUE.id,
  key: ISSUE.key,
  summary: ISSUE.summary,
  project: ISSUE.project,
  type: ISSUE.type,
  status: ISSUE.status,
  priority: ISSUE.priority,
  assignee: ISSUE.assignee,
  updatedAt: ISSUE.updatedAt,
};

/**
 * Animations finished and fonts loaded: axe then measures the final colors
 * and tells icon ligatures (`dark_mode`) from text only once the icon font
 * is applied.
 */
async function animationsDone(page: Page) {
  await page.evaluate(() => document.fonts.ready);
  await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== 'running'));
}

/** The API of tickets; what was posted, kept. */
async function mockTickets(page: Page, mine = [SUMMARY]) {
  const posted: unknown[] = [];
  await page.route('**/api/v1/me/issues**', (route) => route.fulfill({ json: { items: mine, nextCursor: null } }));
  await page.route('**/api/v1/nodes?**', (route) => route.fulfill({ json: { items: [PROJECT], nextCursor: null } }));
  await page.route('**/api/v1/projects/PLAT/issue-form**', (route) => route.fulfill({ json: FORM }));
  await page.route('**/api/v1/projects/PLAT/people**', (route) => route.fulfill({ json: [{ id: 'a2', name: 'Camille Martin' }] }));
  await page.route('**/api/v1/projects/PLAT/issues', (route) => {
    const body = route.request().postDataJSON() as { fields: Record<string, unknown> };
    posted.push(body);
    return body.fields['f-app'] ? route.fulfill({ status: 201, json: ISSUE }) : route.fulfill(problem(400, 'validation_failed', [{ field: 'fields.f-app', code: 'required' }]));
  });
  await page.route('**/api/v1/issues/PLAT-142**', (route) => route.fulfill({ json: ISSUE }));
  await page.route(`**/api/v1/nodes/${PROJECT.id}/my-permissions`, (route) => route.fulfill({ json: ['view', 'create_ticket'] }));
  return posted;
}

test('a member creates a ticket from "My tickets", then lands on its page', async ({ page }) => {
  const posted = await mockTickets(page, []);
  await page.goto('/tickets');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('My tickets');
  await expect(page.getByText('No ticket yet')).toBeVisible();
  await page.getByRole('button', { name: 'New ticket' }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('combobox', { name: 'Project' }).click();
  await page.getByRole('option', { name: 'Identity platform (PLAT)' }).click();
  await expect(dialog.getByRole('combobox', { name: 'Type' })).toContainText('Story');
  await dialog.getByLabel('Summary').fill('Migrate the authentication to OIDC');
  await dialog.getByRole('textbox', { name: 'Description' }).click();
  await page.keyboard.type('Replace the local accounts.');
  // Formatted from the toolbar.
  await dialog.getByRole('button', { name: 'Bulleted list' }).click();
  await dialog.getByRole('button', { name: 'Create' }).click();
  // Required by the type: said next to it, nothing sent.
  await expect(dialog.getByText('This field is required.')).toBeVisible();
  expect(posted).toEqual([]);
  await dialog.getByRole('combobox', { name: 'Application' }).click();
  await page.getByRole('option', { name: 'Mail' }).click();
  await dialog.getByRole('button', { name: 'Create' }).click();

  await expect(page).toHaveURL(/\/issues\/PLAT-142$/);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Migrate the authentication to OIDC');
  expect(posted).toEqual([
    expect.objectContaining({
      typeId: 't-story',
      priority: 'medium',
      description: {
        type: 'doc',
        content: [{ type: 'bulletList', content: [{ type: 'listItem', content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Replace the local accounts.' }] }] }] }],
      },
      fields: { 'f-app': 'v-mail' },
    }),
  ]);
});

test('a link in the description cannot point to a script', async ({ page }) => {
  await mockTickets(page);
  await page.goto('/tickets');
  await page.getByRole('button', { name: 'New ticket' }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('combobox', { name: 'Project' }).click();
  await page.getByRole('option', { name: 'Identity platform (PLAT)' }).click();
  await dialog.getByRole('textbox', { name: 'Description' }).click();
  await page.keyboard.type('docs');
  await page.keyboard.press('ControlOrMeta+a');
  await dialog.getByRole('button', { name: 'Link', exact: true }).click();
  await dialog.getByLabel('Link address').fill('javascript:alert(1)');
  await dialog.getByLabel('Link address').press('Enter');
  await expect(dialog.getByRole('alert')).toContainText('Enter a web address');
  await expect(dialog.getByRole('textbox', { name: 'Description' }).locator('a')).toHaveCount(0);
  await dialog.getByLabel('Link address').fill('https://example.org/docs');
  await dialog.getByLabel('Link address').press('Enter');
  await expect(dialog.getByRole('textbox', { name: 'Description' }).locator('a')).toHaveAttribute('href', 'https://example.org/docs');
});

for (const theme of ['light', 'dark'] as const) {
  for (const language of ['fr', 'en'] as const) {
    test.describe(`accessibility of tickets, ${theme} mode, ${language}`, () => {
      test.use({ theme, language });

      test('"My tickets", the creation dialog and a ticket have no axe-core violation', async ({ page }) => {
        await mockTickets(page);
        await page.goto('/tickets');
        await expect(page.getByRole('link', { name: 'PLAT-142' })).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);

        await page.locator('button.create').click();
        const dialog = page.getByRole('dialog');
        await dialog.locator('mat-select[formcontrolname="project"]').click();
        await page.getByRole('option', { name: 'Identity platform (PLAT)' }).click();
        await expect(dialog.locator('app-field-input')).toHaveCount(3);
        await dialog.locator('button.confirm').click();
        await expect(dialog.locator('mat-error').first()).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
        await page.keyboard.press('Escape');

        await page.goto('/issues/PLAT-142');
        await expect(page.getByRole('heading', { level: 1 })).toHaveText('Migrate the authentication to OIDC');
        await expect(page.locator('ui-rich-text a')).toHaveText('OIDC');
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
      });
    });
  }
}
