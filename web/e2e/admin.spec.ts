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

test('the administration opens on its authentication tab', async ({ page }) => {
  await mockAuthenticationSettings(page);
  await page.goto('/admin');
  await expect(page).toHaveURL(/\/admin\/authentication$/);
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
