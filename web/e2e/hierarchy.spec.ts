import type { Page } from '@playwright/test';

import { SESSION, accessibilityViolations, expect, test } from './fixtures';

const PROGRAM = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7c02',
  type: 'program',
  name: 'IT Modernization',
  key: null,
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

const PROJECT = {
  ...PROGRAM,
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7c03',
  type: 'project',
  name: 'Identity platform',
  key: 'PLAT',
  parents: [{ id: PROGRAM.id, type: 'program', name: 'IT Modernization' }],
};

/** A confidential project the member has no access to, under the program. */
const VEILED = {
  veiled: true,
  id: '5d0c2c1e-8a3b-8f4e-9c1d-2b3a4c5d6e7f',
  type: 'project',
  parents: [{ id: PROGRAM.id, type: 'program', name: 'IT Modernization' }],
};

/** The aggregate of the program: three projects, one of them confidential, some tickets late. */
const AGGREGATE = {
  projects: 3,
  confidentialProjects: 1,
  plannedMinutes: 142_800,
  capacityMinutes: 166_000,
  capacityUsedPercent: 86,
  openTickets: 168,
  lateTickets: 11,
  progressPercent: 46,
  confidentialOpenTickets: 38,
};

/** The rows of the hierarchy tree: the program at the top, the project and a veiled one under it. */
const PROGRAM_ROW = {
  id: PROGRAM.id,
  type: 'program',
  name: PROGRAM.name,
  key: null,
  health: 'at_risk',
  owners: [{ accountId: 'a2', displayName: 'Camille Martin' }],
  weightPercent: null,
  otherParents: [],
  hasChildren: true,
  aggregate: AGGREGATE,
};

const PROJECT_ROW = {
  ...PROGRAM_ROW,
  id: PROJECT.id,
  type: 'project',
  name: PROJECT.name,
  key: 'PLAT',
  health: 'on_track',
  weightPercent: 100,
  otherParents: [{ id: 'p2', type: 'program', name: 'Compliance' }],
  hasChildren: false,
  aggregate: { ...AGGREGATE, projects: 1, confidentialProjects: 0, plannedMinutes: 37_200, lateTickets: 0, confidentialOpenTickets: 0 },
};

const VEILED_ROW = { veiled: true, id: VEILED.id, type: 'project', weightPercent: 100, plannedMinutes: 36_000 };

/** The hierarchy's API, as a tenant admin sees it, recording the creations. */
async function mockHierarchy(page: Page, top: object[] = []) {
  const created: unknown[] = [];
  await page.route('**/api/v1/auth/session', (route) =>
    route.fulfill({ json: { ...SESSION, account: { ...SESSION.account, tenantRole: 'tenant_admin' } } }),
  );
  // One level at a time: the top, or what is under the program.
  await page.route('**/api/v1/hierarchy*', (route) => {
    const under = new URL(route.request().url()).searchParams.get('under');
    return route.fulfill({ json: under === PROGRAM.id ? [PROJECT_ROW, VEILED_ROW] : under ? [] : top });
  });
  await page.route('**/api/v1/nodes', (route) => {
    const body = route.request().postDataJSON() as { type: string };
    created.push(body);
    return route.fulfill({ status: 201, json: body.type === 'program' ? PROGRAM : PROJECT });
  });
  await page.route(`**/api/v1/nodes/${PROGRAM.id}`, (route) => route.fulfill({ json: PROGRAM }));
  await page.route(`**/api/v1/nodes/${PROGRAM.id}/aggregate`, (route) => route.fulfill({ json: AGGREGATE }));
  await page.route(`**/api/v1/nodes/${PROGRAM.id}/my-permissions`, (route) => route.fulfill({ json: [] }));
  await page.route(`**/api/v1/nodes/${PROJECT.id}`, (route) => route.fulfill({ json: PROJECT }));
  return created;
}

/** The links of the project and who has access to it, for a member who administers it. */
async function mockLinks(page: Page) {
  const node = `**/api/v1/nodes/${PROJECT.id}`;
  const manager = { id: 'r3', label: 'Project manager', labelTranslations: { fr: 'Chef de projet', en: 'Project manager' } };
  const reader = { id: 'r1', label: 'Reader', labelTranslations: { fr: 'Lecteur', en: 'Reader' } };
  await page.route(`${node}/parents`, (route) =>
    route.fulfill({ json: [{ id: PROGRAM.id, type: 'program', name: 'IT Modernization', key: null, weightPercent: 40, parents: [] }] }),
  );
  await page.route(`${node}/children`, (route) =>
    route.fulfill({
      json: [
        { id: 'c1', type: 'project', name: 'Cyber permissions', key: 'CYBP', weightPercent: 100, parents: [] },
        { ...VEILED, parents: [] },
      ],
    }),
  );
  await page.route(`${node}/parent-candidates?*`, (route) =>
    route.fulfill({
      json: [
        { id: 'p3', type: 'program', name: 'Cybersecurity', key: null, parents: [], refusal: null },
        { id: 'c1', type: 'project', name: 'Cyber permissions', key: 'CYBP', parents: [], refusal: 'cycle' },
      ],
    }),
  );
  await page.route(`${node}/my-permissions`, (route) =>
    route.fulfill({ json: ['view', 'manage_members', 'administer_node'] }),
  );
  await page.route(`${node}/access`, (route) =>
    route.fulfill({
      json: [
        {
          grantId: 'g1',
          holder: { kind: 'member', id: 'a2', name: 'Camille Martin' },
          role: manager,
          origin: { id: PROJECT.id, type: 'project', name: PROJECT.name },
          direct: true,
        },
        {
          grantId: 'g2',
          holder: { kind: 'group', id: 'gr1', name: 'Managers' },
          role: reader,
          origin: { id: PROGRAM.id, type: 'program', name: PROGRAM.name },
          direct: false,
        },
      ],
    }),
  );
  await page.route(`${node}/grant-candidates*`, (route) =>
    route.fulfill({
      json: [
        { kind: 'group', id: 'gr1', name: 'Managers', email: null },
        { kind: 'member', id: 'a4', name: 'Manon Petit', email: 'manon.petit@acme.example' },
      ],
    }),
  );
  await page.route(`${node}/grantable-roles`, (route) => route.fulfill({ json: [reader, manager] }));
}

async function animationsDone(page: Page) {
  await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== 'running'));
}

test('a program is created at the root, then a project under it, with the keyboard', async ({ page }) => {
  const created = await mockHierarchy(page);
  await page.goto('/portfolios');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Portfolios');
  await page.getByRole('button', { name: 'New node' }).click();
  const dialog = page.getByRole('dialog', { name: 'New node' });
  await dialog.getByRole('combobox', { name: 'Type' }).focus();
  await page.keyboard.press('Enter');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await dialog.getByLabel('Name').fill('IT Modernization');
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(page).toHaveURL(new RegExp(`/nodes/${PROGRAM.id}/general$`));
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('IT Modernization · Settings');

  await page.getByRole('button', { name: 'Add a child' }).click();
  const child = page.getByRole('dialog', { name: 'Add a child to IT Modernization' });
  await child.getByRole('combobox', { name: 'Type' }).click();
  await expect(page.getByRole('option')).toHaveText(['Program', 'Project']);
  await page.getByRole('option', { name: 'Project' }).click();
  await child.getByLabel('Name').fill('Identity platform');
  await child.getByLabel('Key').fill('plat');
  await child.getByRole('button', { name: 'Create' }).click();
  await expect(page).toHaveURL(new RegExp(`/nodes/${PROJECT.id}/general$`));
  await expect(page.getByRole('navigation', { name: 'Breadcrumb' })).toContainText('IT Modernization');
  expect(created).toEqual([
    { type: 'program', name: 'IT Modernization', key: null, startDate: null, endDate: null, ownerIds: [], parentId: null },
    { type: 'project', name: 'Identity platform', key: 'PLAT', startDate: null, endDate: null, ownerIds: [], parentId: PROGRAM.id },
  ]);
});

for (const theme of ['light', 'dark'] as const) {
  for (const language of ['fr', 'en'] as const) {
    test.describe(`accessibility of the hierarchy, ${theme} mode, ${language}`, () => {
      test.use({ theme, language });

      test('the list, the creation dialog, the view of a node and its settings have no axe-core violation', async ({ page }) => {
        await mockHierarchy(page, [PROGRAM_ROW]);
        await page.goto('/portfolios');
        await expect(page.getByRole('row')).toHaveCount(2);
        // The program expands to its level with the keyboard.
        const toggle = page.locator('button.toggle');
        await toggle.focus();
        await page.keyboard.press('Enter');
        await expect(toggle).toHaveAttribute('aria-expanded', 'true');
        await expect(page.getByRole('row')).toHaveCount(4);
        await expect(page.locator('app-veiled-name')).toHaveCount(1);
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'list').toEqual([]);

        // The view of the program: its indicators (late tickets in the alert colors) and its tree.
        await page.goto(`/nodes/${PROGRAM.id}`);
        await expect(page.locator('.indicator')).toHaveCount(4);
        await expect(page.locator('.indicator.late.alert')).toBeVisible();
        await expect(page.getByRole('row')).toHaveCount(3);
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'view').toEqual([]);
        await page.goto('/portfolios');

        await page.locator('button.create').click();
        await expect(page.getByRole('dialog')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'dialog').toEqual([]);
        await page.keyboard.press('Escape');

        await page.goto(`/nodes/${PROJECT.id}/general`);
        await expect(page.locator('app-node-general-tab form')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'settings').toEqual([]);
      });

      test('the attachments, the confidentiality, the access and the grant dialog have no axe-core violation', async ({
        page,
      }) => {
        await mockHierarchy(page);
        await mockLinks(page);
        await page.goto(`/nodes/${PROJECT.id}/links`);
        await expect(page.locator('app-node-access-card li')).toHaveCount(2);
        await expect(page.locator('app-weight-field')).toHaveCount(1);
        await expect(page.getByRole('switch')).toBeVisible();
        await expect(page.getByRole('img', { name: language === 'fr' ? 'Verrouillé' : 'Locked' })).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'attachments and access').toEqual([]);

        await page.locator('button.add-parent').click();
        await expect(page.locator('#parent-search li')).toHaveCount(2);
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'parent search').toEqual([]);

        await page.locator('button.add-grant').click();
        await expect(page.getByRole('dialog')).toBeVisible();
        await expect(page.getByRole('option')).toHaveCount(2);
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'grant dialog').toEqual([]);
      });
    });
  }
}
