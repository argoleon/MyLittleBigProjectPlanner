import type { Page } from '@playwright/test';

import { SESSION, accessibilityViolations, expect, test } from './fixtures';

/**
 * Project configurations (L04-US01), with the API mocked: a configuration
 * admin creates one and publishes a draft after reviewing its projects;
 * axe-core on every screen, in both themes and languages.
 */

const DEFAULT = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a01',
  name: 'Default configuration',
  nameTranslations: { fr: 'Configuration par défaut', en: 'Default configuration' },
  isDefault: true,
  projectCount: 3,
  published: { number: 1, publishedAt: '2026-11-10T09:00:00Z' },
  draft: null,
  version: 1,
};

const SOFTWARE = {
  ...DEFAULT,
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a02',
  name: 'Software development',
  nameTranslations: { fr: 'Développement logiciel', en: 'Software development' },
  isDefault: false,
  projectCount: 6,
  published: { number: 2, publishedAt: '2026-11-12T09:00:00Z' },
  draft: { number: 3, changeCount: 2, createdAt: '2026-11-13T08:00:00Z' },
  version: 5,
};

const SUPPORT = {
  ...SOFTWARE,
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a03',
  name: 'Support',
  nameTranslations: { fr: 'Support', en: 'Support' },
  projectCount: 0,
  published: { number: 1, publishedAt: '2026-11-13T09:00:00Z' },
  draft: null,
  version: 1,
};

const PROJECTS = [
  { id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7c03', name: 'Identity platform', key: 'PLAT', archived: false },
  { id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7c04', name: 'Legacy CRM', key: 'CRM', archived: true },
  { veiled: true, id: '5d0c2c1e-8a3b-8f4e-9c1d-2b3a4c5d6e7f' },
];

const PROJECT = {
  id: PROJECTS[0].id,
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

const TYPE = (id: string, key: string, en: string, color: string, icon: string, parentIds: string[], level: number, isSubtask = false) => ({
  id,
  key,
  label: en,
  labelTranslations: { fr: en, en },
  icon,
  color,
  isSubtask,
  parentIds,
  workflowId: 'w1',
  level,
  version: 1,
});

const STATUS = (id: string, key: string, fr: string, en: string, category: string, isInitial = false) => ({
  id,
  key,
  label: en,
  labelTranslations: { fr, en },
  portalLabel: null,
  portalLabelTranslations: {},
  category,
  isInitial,
  version: 1,
});

/** The standard workflow of mockup 11, followed by every type. */
const STANDARD = {
  id: 'w1',
  key: 'standard',
  label: 'Standard workflow',
  labelTranslations: { fr: 'Workflow standard', en: 'Standard workflow' },
  issueTypeIds: ['t1', 't2', 't3', 't4', 't5'],
  statuses: [
    STATUS('s1', 'todo', 'À faire', 'To do', 'todo', true),
    STATUS('s2', 'in_progress', 'En cours', 'In progress', 'in_progress'),
    STATUS('s3', 'in_review', 'En revue', 'In review', 'in_progress'),
    STATUS('s4', 'done', 'Terminé', 'Done', 'done'),
    STATUS('s5', 'cancelled', 'Annulé', 'Cancelled', 'done'),
  ],
  transitions: [
    { id: 'x1', name: 'Start', nameTranslations: { fr: 'Démarrer', en: 'Start' }, fromId: 's1', toId: 's2', version: 1 },
    { id: 'x2', name: 'Send to review', nameTranslations: { fr: 'Passer en revue', en: 'Send to review' }, fromId: 's2', toId: 's3', version: 1 },
    { id: 'x3', name: 'Reject', nameTranslations: { fr: 'Refuser', en: 'Reject' }, fromId: 's3', toId: 's2', version: 1 },
    { id: 'x4', name: 'Validate', nameTranslations: { fr: 'Valider', en: 'Validate' }, fromId: 's3', toId: 's4', version: 1 },
    { id: 'x5', name: 'Cancel', nameTranslations: { fr: 'Annuler', en: 'Cancel' }, fromId: null, toId: 's5', version: 1 },
  ],
  problems: [],
  version: 1,
};

const TYPES = [
  TYPE('t1', 'epic', 'Epic', 'epic', 'bolt', [], 1),
  TYPE('t2', 'story', 'Story', 'story', 'bookmark', ['t1'], 2),
  TYPE('t3', 'task', 'Task', 'task', 'task_alt', ['t1'], 2),
  TYPE('t4', 'bug', 'Bug', 'bug', 'bug_report', ['t1'], 2),
  TYPE('t5', 'subtask', 'Sub-task', 'task', 'subdirectory_arrow_right', ['t2', 't3', 't4'], 3, true),
];

const VALUE = (id: string, fr: string, en: string, published = true) => ({
  id,
  label: en,
  labelTranslations: { fr, en },
  active: true,
  published,
  children: [],
});

/** Mockup 12: Application, a single select with its rules, and the two system fields. */
const FIELDS = [
  {
    id: 'f1',
    key: 'application',
    label: 'Application',
    labelTranslations: { fr: 'Application', en: 'Application' },
    kind: 'single_select',
    options: { values: [VALUE('v1', 'Annuaire', 'Directory'), VALUE('v2', 'Messagerie', 'Mail'), VALUE('v3', 'ERP', 'ERP', false)] },
    isSystem: false,
    allTypes: false,
    issueTypeIds: ['t2', 't3', 't4'],
    rules: [
      { issueTypeId: 't3', statusId: null, mode: 'hidden' },
      { issueTypeId: 't4', statusId: null, mode: 'required' },
      { issueTypeId: 't2', statusId: 's4', mode: 'required' },
    ],
    version: 1,
  },
  {
    id: 'f2',
    key: 'story_points',
    label: 'Points',
    labelTranslations: { fr: 'Points', en: 'Points' },
    kind: 'number',
    options: { decimals: 1 },
    isSystem: true,
    allTypes: false,
    issueTypeIds: ['t2', 't3'],
    rules: [],
    version: 1,
  },
  {
    id: 'f3',
    key: 'remaining',
    label: 'Remaining work',
    labelTranslations: { fr: 'Reste à faire', en: 'Remaining work' },
    kind: 'number',
    options: { decimals: 2, unit: 'hours' },
    isSystem: true,
    allTypes: true,
    issueTypeIds: [],
    rules: [],
    version: 1,
  },
];

/** The configurations' API for a configuration admin, recording what is created and published. */
async function mockConfigurations(page: Page) {
  const sent: { created: unknown[]; published: (string | null)[]; ordered: unknown[]; fields: unknown[] } = {
    created: [],
    published: [],
    ordered: [],
    fields: [],
  };
  await page.route('**/api/v1/auth/session', (route) =>
    route.fulfill({ json: { ...SESSION, account: { ...SESSION.account, tenantRole: 'configuration_admin' } } }),
  );
  await page.route('**/api/v1/project-configs?*', (route) => route.fulfill({ json: { items: [DEFAULT, SOFTWARE], nextCursor: null } }));
  await page.route('**/api/v1/project-configs', (route) => {
    sent.created.push(route.request().postDataJSON());
    return route.fulfill({ status: 201, json: SUPPORT });
  });
  for (const config of [DEFAULT, SOFTWARE, SUPPORT]) {
    await page.route(`**/api/v1/project-configs/${config.id}`, (route) => route.fulfill({ json: config }));
    await page.route(`**/api/v1/project-configs/${config.id}/projects`, (route) =>
      route.fulfill({ json: { items: config.projectCount ? PROJECTS : [] } }),
    );
  }
  await page.route(`**/api/v1/project-configs/*/issue-types`, (route) => route.fulfill({ json: { stage: 'published', items: TYPES } }));
  await page.route(`**/api/v1/project-configs/*/workflows`, (route) => route.fulfill({ json: { stage: 'published', items: [STANDARD] } }));
  await page.route(`**/api/v1/project-configs/*/fields`, (route) => route.fulfill({ json: { stage: 'published', items: FIELDS } }));
  await page.route(`**/api/v1/project-configs/*/fields/*`, (route) => {
    const body = route.request().postDataJSON() as object;
    sent.fields.push(body);
    return route.fulfill({ json: { ...FIELDS[0], ...body, version: 2 } });
  });
  await page.route(`**/api/v1/project-configs/*/issue-types/order`, (route) => {
    const ids = (route.request().postDataJSON() as { ids: string[] }).ids;
    sent.ordered.push(ids);
    return route.fulfill({ json: { stage: 'draft', items: ids.map((id) => TYPES.find((t) => t.id === id)) } });
  });
  await page.route(`**/api/v1/project-configs/${SOFTWARE.id}/publish-impact`, (route) =>
    route.fulfill({ json: { projects: PROJECTS, removedTypes: [], removedStatuses: [], workflowProblems: [], needsMigration: false, version: 5 } }),
  );
  await page.route(`**/api/v1/project-configs/${SOFTWARE.id}/publish`, (route) => {
    sent.published.push(route.request().headers()['if-match'] ?? null);
    return route.fulfill({
      json: { ...SOFTWARE, published: { number: 3, publishedAt: '2026-11-13T10:00:00Z' }, draft: null, version: 6 },
    });
  });
  await page.route(`**/api/v1/nodes/${PROJECT.id}`, (route) => route.fulfill({ json: PROJECT }));
  await page.route(`**/api/v1/nodes/${PROJECT.id}/configuration`, (route) =>
    route.fulfill({ json: { id: SOFTWARE.id, name: SOFTWARE.name, projectCount: 6, isDefault: false } }),
  );
  return sent;
}

async function animationsDone(page: Page) {
  await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== 'running'));
}

test('a configuration admin creates a configuration, then publishes a draft after reviewing its projects', async ({ page }) => {
  const sent = await mockConfigurations(page);
  await page.goto('/admin');
  await expect(page).toHaveURL(/\/admin\/configurations$/);
  await expect(page.getByRole('tab')).toHaveText(['Project configurations']);
  await expect(page.getByRole('row')).toHaveCount(3);
  await expect(page.getByRole('row').nth(2)).toContainText('Shared by 6 projects');
  await expect(page.getByRole('row').nth(2)).toContainText('Unpublished draft · 2 changes');

  await page.getByRole('button', { name: 'New configuration' }).click();
  const dialog = page.getByRole('dialog', { name: 'New project configuration' });
  // Opened: its first field has the focus, which would otherwise move while typing.
  await expect(dialog.getByLabel('Name (Français)')).toBeFocused();
  await dialog.getByLabel('Name (Français)').fill('Support');
  await dialog.getByLabel('Name (English)').fill('Support');
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(page).toHaveURL(new RegExp(`/admin/configurations/${SUPPORT.id}/types$`));
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Support');
  await expect(page.getByRole('button', { name: 'Publish the changes' })).toBeDisabled();
  expect(sent.created).toEqual([{ nameTranslations: { fr: 'Support', en: 'Support' } }]);

  // The shared configuration: its projects first, then its draft published.
  await page.goto(`/admin/configurations/${SOFTWARE.id}/projects`);
  await expect(page.getByRole('tab', { name: 'Projects (6)' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByRole('link', { name: 'Identity platform' })).toBeVisible();
  await page.getByRole('button', { name: 'Publish the changes' }).click();
  const publish = page.getByRole('dialog', { name: 'Publish “Software development”?' });
  await expect(publish.getByRole('listitem')).toHaveCount(3);
  await publish.getByRole('button', { name: 'Publish' }).click();
  await expect(page.getByRole('status')).toHaveText('Version 3 is published to its 6 projects.');
  expect(sent.published).toEqual(['"5"']);
});

test('the issue types are reordered with the keyboard alone', async ({ page }) => {
  const sent = await mockConfigurations(page);
  await page.goto(`/admin/configurations/${SOFTWARE.id}/types`);
  await expect(page.getByRole('row')).toHaveCount(6);
  const down = page.getByRole('button', { name: 'Move Bug down' });
  await down.focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('status')).toHaveText('Bug moved to position 5.');
  await expect(page.locator('code.key')).toHaveText(['epic', 'story', 'task', 'subtask', 'bug']);
  expect(sent.ordered).toEqual([['t1', 't2', 't3', 't5', 't4']]);
  // Up again, from the keyboard focus.
  await page.getByRole('button', { name: 'Move Bug up' }).focus();
  await page.keyboard.press('Enter');
  await expect(page.locator('code.key')).toHaveText(['epic', 'story', 'task', 'bug', 'subtask']);
});

test('the rules of a field are set with the keyboard alone', async ({ page }) => {
  const sent = await mockConfigurations(page);
  await page.goto(`/admin/configurations/${SOFTWARE.id}/fields`);
  await page.getByRole('button', { name: 'Edit Application' }).click();
  const panel = page.getByRole('region', { name: 'Edit “Application”' });
  // Task at creation: Hidden, then Visible, then Required.
  const task = panel.getByRole('button', { name: 'Task, Creation: Hidden' });
  await task.focus();
  await page.keyboard.press('Enter');
  await expect(panel.getByRole('button', { name: 'Task, Creation: Visible' })).toBeFocused();
  await page.keyboard.press('Space');
  await expect(panel.getByRole('button', { name: 'Task, Creation: Required' })).toBeFocused();
  await panel.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByRole('status')).toHaveText('Field “Application” saved in the draft.');
  expect(sent.fields).toEqual([
    expect.objectContaining({
      rules: [
        { issueTypeId: 't4', statusId: null, mode: 'required' },
        { issueTypeId: 't2', statusId: 's4', mode: 'required' },
        { issueTypeId: 't3', statusId: null, mode: 'required' },
      ],
    }),
  ]);
});

for (const theme of ['light', 'dark'] as const) {
  for (const language of ['fr', 'en'] as const) {
    test.describe(`accessibility of the project configurations, ${theme} mode, ${language}`, () => {
      test.use({ theme, language });

      test('the list, the creation dialog, a configuration, its publication and a project tab have no axe-core violation', async ({
        page,
      }) => {
        await mockConfigurations(page);
        await page.goto('/admin/configurations');
        await expect(page.getByRole('row')).toHaveCount(3);
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'list').toEqual([]);

        await page.locator('button.create').click();
        await expect(page.getByRole('dialog')).toBeVisible();
        await page.getByRole('dialog').locator('button.confirm').click();
        await expect(page.getByRole('dialog').locator('mat-error')).toHaveCount(1);
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'creation dialog').toEqual([]);
        await page.keyboard.press('Escape');

        await page.goto(`/admin/configurations/${SOFTWARE.id}/projects`);
        await expect(page.getByRole('row')).toHaveCount(4);
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'configuration').toEqual([]);

        await page.locator('button.publish').click();
        await expect(page.getByRole('dialog').getByRole('listitem')).toHaveCount(3);
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'publication').toEqual([]);
        await page.keyboard.press('Escape');

        await page.goto(`/admin/configurations/${SOFTWARE.id}/types`);
        await expect(page.locator('app-types-tab tr[mat-row]')).toHaveCount(5);
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'types').toEqual([]);
        await page.locator('app-types-tab button.edit').nth(4).click();
        await expect(page.locator('app-type-editor')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'type panel').toEqual([]);

        await page.goto(`/admin/configurations/${SOFTWARE.id}/workflows`);
        await expect(page.locator('app-workflow-graph g[data-category]')).toHaveCount(5);
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'workflows').toEqual([]);
        await page.locator('button.add-status').click();
        await expect(page.getByRole('dialog')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'status dialog').toEqual([]);
        await page.keyboard.press('Escape');

        await page.goto(`/admin/configurations/${SOFTWARE.id}/fields`);
        await expect(page.locator('app-fields-tab tr[mat-row]')).toHaveCount(3);
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'fields').toEqual([]);
        await page.locator('app-fields-tab button.edit').first().click();
        await expect(page.locator('app-rules-grid button.mode')).toHaveCount(3 * 6);
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'field panel').toEqual([]);

        await page.goto(`/admin/configurations/${SOFTWARE.id}/screens`);
        await expect(page.locator('app-config-section-tab')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'section').toEqual([]);

        await page.goto(`/nodes/${PROJECT.id}/configuration`);
        await expect(page.locator('app-node-configuration-tab .uses')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'project configuration tab').toEqual([]);
      });
    });
  }
}
