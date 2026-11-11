import type { Page } from '@playwright/test';

import { accessibilityViolations, expect, test } from './fixtures';

const PROVIDER = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a8c',
  displayName: 'Entra ID',
  issuer: 'https://login.microsoftonline.com/acme/v2.0',
  clientId: 'client-1',
  emailClaim: 'email',
  enabled: true,
  redirectUri: 'https://acme.example.org/api/v1/auth/oidc/callback',
  version: 1,
  createdAt: '2026-11-10T09:00:00Z',
  updatedAt: '2026-11-10T09:00:00Z',
};

async function mockAuthenticationSettings(page: Page) {
  await page.route('**/api/v1/tenant/auth-policy', (route) =>
    route.fulfill({
      json: {
        internalAccountsRequireSso: true,
        localAccountsRequireMfa: true,
        portalPasswordEnabled: true,
        portalMagicLinkEnabled: false,
      },
      headers: { ETag: '"1"' },
    }),
  );
  await page.route('**/api/v1/tenant/oidc-providers', (route) => route.fulfill({ json: [PROVIDER] }));
}

async function animationsDone(page: Page) {
  await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== 'running'));
}

const MEMBER = {
  accountId: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a01',
  email: 'lucas.bernard@acme.example',
  displayName: 'Lucas Bernard',
  accountType: 'internal',
  tenantRole: 'tenant_admin',
  localAccount: false,
  status: 'active',
  mfaEnabled: true,
  groups: [{ id: 'g1', name: 'Platform' }],
  version: 1,
};

const INVITATION = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a10',
  email: 'p.durand@client.example',
  accountType: 'external',
  tenantRole: 'member',
  status: 'pending',
  expiresAt: '2026-11-12T09:00:00Z',
  invitedBy: 'Lucas Bernard',
  createdAt: '2026-11-05T09:00:00Z',
  version: 1,
};

async function mockMembers(page: Page) {
  await page.route('**/api/v1/tenant/members?*', (route) => route.fulfill({ json: { items: [MEMBER], nextCursor: null } }));
  await page.route('**/api/v1/tenant/invitations?*', (route) =>
    route.fulfill({ json: { items: [INVITATION], nextCursor: null } }),
  );
  await page.route('**/api/v1/tenant/groups?*', (route) =>
    route.fulfill({
      json: { items: [{ id: 'g1', name: 'Platform', description: 'Platform team', memberCount: 1, version: 1 }], nextCursor: null },
    }),
  );
}

test('the administration opens on its members tab', async ({ page }) => {
  await mockMembers(page);
  await page.goto('/admin');
  await expect(page).toHaveURL(/\/admin\/members$/);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Administration');
  await expect(page.getByRole('tab', { name: 'Members' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByRole('row')).toHaveCount(3);
  await expect(page.getByRole('row').nth(1)).toContainText('p.durand@client.example');
  await expect(page.getByRole('row').nth(2)).toContainText('Lucas Bernard');
});

test('the members table is usable with the keyboard alone', async ({ page }) => {
  await mockMembers(page);
  await page.goto('/admin/members');
  await expect(page.getByRole('row')).toHaveCount(3);
  const actions = page.getByRole('button', { name: 'Actions for Lucas Bernard' });
  await actions.focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('menuitem', { name: 'Change access' })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('dialog')).toContainText('Access of Lucas Bernard');
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(actions).toBeFocused();
});

test('the authentication tab shows the policy and the providers', async ({ page }) => {
  await mockAuthenticationSettings(page);
  await page.goto('/admin/authentication');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Administration');
  // Routed tabs with a panel: role "tab", selected when active.
  await expect(page.getByRole('tab', { name: 'Authentication' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByRole('switch', { name: /Internal accounts sign in/ })).toBeChecked();
  await expect(page.getByRole('switch', { name: /Portal: sign-in with a link/ })).not.toBeChecked();
  await expect(page.locator('.provider-name')).toHaveText('Entra ID');
});

for (const theme of ['light', 'dark'] as const) {
  for (const language of ['fr', 'en'] as const) {
    test.describe(`accessibility of the authentication settings, ${theme} mode, ${language}`, () => {
      test.use({ theme, language });

      test('the tab, the provider dialog and the sign-in buttons have no axe-core violation', async ({ page }) => {
        await mockAuthenticationSettings(page);
        await page.goto('/admin/authentication');
        await expect(page.locator('.provider-name')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'tab').toEqual([]);

        await page.locator('button[aria-label]').filter({ has: page.locator('mat-icon', { hasText: 'edit' }) }).click();
        await expect(page.getByRole('dialog')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'dialog').toEqual([]);
      });

      test('the members and groups tabs and the invite dialog have no axe-core violation', async ({ page }) => {
        await mockMembers(page);
        await page.goto('/admin/members');
        await expect(page.getByRole('row')).toHaveCount(3);
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'members').toEqual([]);

        await page.locator('button.invite').click();
        await expect(page.getByRole('dialog')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'invite dialog').toEqual([]);
        await page.keyboard.press('Escape');

        await page.goto('/admin/groups');
        await expect(page.getByRole('row')).toHaveCount(2);
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'groups').toEqual([]);
      });

      test('the invitation page has no axe-core violation', async ({ page }) => {
        await page.route('**/api/v1/auth/session', (route) =>
          route.fulfill({ status: 401, contentType: 'application/problem+json', body: '{"code":"unauthenticated"}' }),
        );
        await page.route('**/api/v1/auth/invitation', (route) =>
          route.fulfill({
            json: {
              email: 'camille.martin@acme.example',
              tenantName: 'Acme Industrie',
              invitedBy: 'Lucas Bernard',
              accountType: 'internal',
              newAccount: true,
              expiresAt: '2026-11-18T09:00:00Z',
            },
          }),
        );
        await page.goto('/accept-invitation?token=t1');
        await expect(page.locator('input[formcontrolname="displayName"]')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
      });

      test('the sign-in page with an identity provider has no axe-core violation', async ({ page }) => {
        await page.route('**/api/v1/auth/session', (route) =>
          route.fulfill({ status: 401, contentType: 'application/problem+json', body: '{"code":"unauthenticated"}' }),
        );
        await page.route('**/api/v1/auth/providers', (route) =>
          route.fulfill({ json: [{ id: PROVIDER.id, displayName: 'Entra ID' }] }),
        );
        await page.goto('/sign-in?error=auth.oidc_no_member');
        await expect(page.locator('a.provider')).toBeVisible();
        await expect(page.getByRole('alert')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
      });
    });
  }
}
