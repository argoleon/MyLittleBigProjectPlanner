import type { Page } from '@playwright/test';

import { SESSION, accessibilityViolations, expect, test } from './fixtures';

// The administration is the tenant admin's (configuration admins only see the
// project configurations). Tests that sign out route the session again after.
test.beforeEach(async ({ page }) => {
  await page.route('**/api/v1/auth/session', (route) =>
    route.fulfill({ json: { ...SESSION, account: { ...SESSION.account, tenantRole: 'tenant_admin' } } }),
  );
});

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

const MANAGER = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b02',
  label: 'Project manager',
  labelTranslations: { fr: 'Chef de projet', en: 'Project manager' },
  permissions: ['view', 'create_ticket', 'edit', 'comment', 'log_time', 'assign_resources', 'force_overload', 'manage_sprints'],
  grantCount: 0,
  version: 2,
};

const READER = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b01',
  label: 'Reader',
  labelTranslations: { fr: 'Lecteur', en: 'Reader' },
  permissions: ['view'],
  grantCount: 4,
  version: 1,
};

async function mockRoles(page: Page) {
  await page.route('**/api/v1/tenant/roles?*', (route) =>
    route.fulfill({ json: { items: [MANAGER, READER], nextCursor: null } }),
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

test('a role is changed with the keyboard alone, then saved', async ({ page }) => {
  await mockRoles(page);
  const saved: object[] = [];
  await page.route(`**/api/v1/tenant/roles/${MANAGER.id}`, async (route) => {
    saved.push({ ifMatch: route.request().headers()['if-match'], body: route.request().postDataJSON() });
    await route.fulfill({ json: { ...MANAGER, permissions: [...MANAGER.permissions, 'manage_members'], version: 3 } });
  });
  await page.goto('/admin/roles');
  await expect(page.getByRole('tab', { name: 'Roles' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByRole('heading', { level: 2 })).toHaveText('Role “Project manager”');
  await expect(page.getByRole('option', { name: /Project manager/ })).toHaveAttribute('aria-selected', 'true');

  const manageMembers = page.getByRole('switch', { name: 'Manage members' });
  await expect(manageMembers).not.toBeChecked();
  await manageMembers.focus();
  await page.keyboard.press('Space');
  await expect(manageMembers).toBeChecked();
  await expect(page.getByText('Unsaved changes')).toBeVisible();
  await page.getByRole('button', { name: 'Save' }).focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('status')).toHaveText('Role Project manager saved.');
  await expect(page.getByText('Unsaved changes')).toHaveCount(0);
  expect(saved).toEqual([
    {
      ifMatch: '"2"',
      body: {
        labelTranslations: { fr: 'Chef de projet', en: 'Project manager' },
        permissions: [...MANAGER.permissions, 'manage_members'],
      },
    },
  ]);

  // A role used by grants cannot be deleted.
  await page.getByRole('option', { name: /Reader/ }).click();
  await expect(page.getByRole('heading', { level: 2 })).toHaveText('Role “Reader”');
  await expect(page.getByRole('button', { name: 'Delete' })).toBeDisabled();
  await expect(page.getByText('Grants using this role: 4.')).toBeVisible();
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

      test('the roles tab has no axe-core violation', async ({ page }) => {
        await mockRoles(page);
        await page.goto('/admin/roles');
        await expect(page.getByRole('switch')).toHaveCount(10);
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'roles').toEqual([]);

        // With unsaved changes and the confirmation to discard them.
        await page.getByRole('switch').last().click();
        await page.getByRole('option').last().click();
        await expect(page.getByRole('dialog')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'discard dialog').toEqual([]);
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
