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

/** The configurations' API for a configuration admin, recording what is created and published. */
async function mockConfigurations(page: Page) {
  const sent: { created: unknown[]; published: (string | null)[] } = { created: [], published: [] };
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
  await page.route(`**/api/v1/project-configs/${SOFTWARE.id}/publish-impact`, (route) =>
    route.fulfill({ json: { projects: PROJECTS, needsMigration: false, version: 5 } }),
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
