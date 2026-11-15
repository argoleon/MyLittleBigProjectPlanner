/**
 * Notifications (L05-US07), the API mocked: the bell's count, the
 * notifications of two organizations filtered and marked read, the
 * preferences saved; accessibility in both themes and languages.
 */
import type { Page } from '@playwright/test';

import { accessibilityViolations, expect, test } from './fixtures';

const now = Date.now();
const ago = (minutes: number) => new Date(now - minutes * 60_000).toISOString();

const notification = (id: string, rest: object) => ({
  id,
  tenant: { id: 'a1b2c3d4-0000-4000-8000-000000000001', slug: 'acme', name: 'Acme Industrie' },
  event: 'mentioned',
  issueId: `issue-${id}`,
  issueKey: 'PLAT-151',
  issueSummary: 'MFA policy',
  actorName: 'Inès Moreau',
  status: null,
  excerpt: '@Lucas can you validate the MFA policy?',
  read: false,
  createdAt: ago(10),
  ...rest,
});

/** Two unread notifications, one of another organization, and one read; what was marked read and saved, kept. */
async function mockNotifications(page: Page) {
  const sent: { read: unknown[]; preferences: unknown[] } = { read: [], preferences: [] };
  let unread = 2;
  await page.route('**/api/v1/me/notifications/unread', (route) =>
    route.fulfill({ json: { total: unread, byTenant: [] } }),
  );
  await page.route('**/api/v1/me/notifications?**', (route) =>
    route.fulfill({
      json: {
        items: [
          notification('n1', {}),
          notification('n2', {
            tenant: { id: 'b1b2c3d4-0000-4000-8000-000000000002', slug: 'services', name: 'Acme Services' },
            event: 'assigned',
            issueKey: 'SUP-1042',
            issueSummary: 'Mail access',
            excerpt: null,
            createdAt: ago(60),
          }),
          notification('n3', { event: 'status_changed', status: 'In progress', excerpt: null, read: true, createdAt: ago(60 * 24 * 3) }),
        ].map((n) => (unread === 0 ? { ...n, read: true } : n)),
        nextCursor: null,
      },
    }),
  );
  await page.route('**/api/v1/me/notifications/read', (route) => {
    sent.read.push(route.request().postDataJSON());
    unread = 0;
    return route.fulfill({ json: { total: 0, byTenant: [] } });
  });
  await page.route('**/api/v1/me/notification-preferences', (route) => {
    if (route.request().method() === 'PUT') {
      sent.preferences.push(route.request().postDataJSON());
      return route.fulfill({ json: route.request().postDataJSON() });
    }
    return route.fulfill({
      json: {
        events: [
          { event: 'created', inApp: true, email: true },
          { event: 'assigned', inApp: true, email: true },
          { event: 'status_changed', inApp: true, email: false },
          { event: 'commented', inApp: true, email: false },
          { event: 'mentioned', inApp: true, email: true },
        ],
        dailyDigest: false,
      },
    });
  });
  await page.route('**/api/v1/me/memberships', (route) =>
    route.fulfill({
      json: [
        { tenantId: 'a1b2c3d4-0000-4000-8000-000000000001', slug: 'acme', name: 'Acme Industrie', accountType: 'internal', tenantRole: 'member', current: true },
        { tenantId: 'b1b2c3d4-0000-4000-8000-000000000002', slug: 'services', name: 'Acme Services', accountType: 'internal', tenantRole: 'member', current: false },
      ],
    }),
  );
  return sent;
}

test('the bell leads to the notifications of both organizations, marked read, and the preferences are saved', async ({ page }) => {
  const sent = await mockNotifications(page);
  await page.goto('/');
  const bell = page.getByRole('link', { name: 'Notifications, 2 unread' });
  await expect(bell).toBeVisible();
  await bell.click();
  await expect(page.getByRole('heading', { level: 1, name: 'Notifications' })).toBeVisible();
  await expect(page.locator('.item .what')).toHaveText([
    'Inès Moreau mentioned you on PLAT-151 (unread)',
    'Inès Moreau assigned SUP-1042 to you (unread)',
    'Inès Moreau moved PLAT-151 to In progress',
  ]);
  await expect(page.locator('.item .elsewhere')).toHaveText(['opens in Acme Services']);
  await page.getByRole('option', { name: 'Assignments' }).click();
  await expect(page.locator('.item')).toHaveCount(1);
  await page.getByRole('option', { name: 'All', exact: true }).click();
  await page.getByRole('button', { name: 'Mark all as read' }).click();
  await expect(page.locator('.item.unread')).toHaveCount(0);
  await expect(page.getByRole('link', { name: 'Notifications', exact: true })).toBeVisible();
  expect(sent.read).toEqual([{}]);

  await page.getByRole('checkbox', { name: 'Status change by email' }).check();
  await page.getByRole('switch', { name: 'Daily email digest' }).click();
  await expect.poll(() => sent.preferences.length).toBe(2);
  expect(sent.preferences[1]).toMatchObject({ dailyDigest: true });
});

for (const theme of ['light', 'dark'] as const) {
  for (const language of ['fr', 'en'] as const) {
    test.describe(`accessibility of notifications, ${theme} mode, ${language}`, () => {
      test.use({ theme, language });

      test('the notifications page and its preferences have no axe-core violation', async ({ page }) => {
        await mockNotifications(page);
        await page.goto('/notifications');
        await expect(page.locator('.item')).toHaveCount(3);
        await expect(page.locator('.choices tbody tr')).toHaveCount(5);
        await page.evaluate(() => document.fonts.ready);
        expect(await accessibilityViolations(page)).toEqual([]);
      });
    });
  }
}
