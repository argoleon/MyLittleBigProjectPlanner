import type { Page } from '@playwright/test';

import { MEMBERSHIP, ME, accessibilityViolations, expect, test } from './fixtures';

const THIS = {
  id: 'a'.repeat(32),
  current: true,
  tenantId: MEMBERSHIP.tenantId,
  tenantName: MEMBERSHIP.name,
  browser: 'Firefox',
  os: 'Linux',
  createdAt: '2026-11-11T08:00:00Z',
  lastSeenAt: '2026-11-11T10:00:00Z',
  expiresAt: '2026-11-18T08:00:00Z',
};

const PHONE = {
  ...THIS,
  id: 'b'.repeat(32),
  current: false,
  browser: 'Safari',
  os: 'iOS',
  createdAt: '2026-11-10T07:00:00Z',
  lastSeenAt: '2026-11-10T19:00:00Z',
};

/** The account's API, recording what the page sends. */
async function mockAccount(page: Page) {
  const sent: { method: string; path: string; body: unknown }[] = [];
  let sessions = [THIS, PHONE];
  await page.route('**/api/v1/account/mfa', (route) =>
    route.fulfill({ json: { enabled: true, backupCodesLeft: 8, requiredByPolicy: false } }),
  );
  await page.route('**/api/v1/me/sessions', (route) => route.fulfill({ json: sessions }));
  await page.route('**/api/v1/me/sessions/*', (route) => {
    sent.push({ method: route.request().method(), path: new URL(route.request().url()).pathname, body: null });
    sessions = [THIS];
    return route.fulfill({ status: 204 });
  });
  await page.route('**/api/v1/me/preferences', (route) => {
    const body = route.request().postDataJSON() as object;
    sent.push({ method: route.request().method(), path: '/api/v1/me/preferences', body });
    return route.fulfill({ json: body });
  });
  await page.route('**/api/v1/me/password', (route) => {
    sent.push({ method: 'POST', path: '/api/v1/me/password', body: route.request().postDataJSON() });
    sessions = [THIS];
    return route.fulfill({ status: 204 });
  });
  return sent;
}

async function animationsDone(page: Page) {
  await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== 'running'));
}

test('the account menu opens My account', async ({ page }) => {
  await mockAccount(page);
  await page.goto('/');
  await page.getByRole('button', { name: 'Account: Lucas Bernard' }).click();
  await page.getByRole('menuitem', { name: 'My account' }).click();
  await expect(page).toHaveURL(/\/account$/);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('My account');
  await expect(page.getByText(`Global account · ${ME.email}`)).toBeVisible();
  await expect(page.getByRole('switch', { name: 'Two-step verification' })).toBeChecked();
  await expect(page.getByText('Devices: 2')).toBeVisible();
});

test('the language is chosen with the keyboard, saved for the account and applied at once', async ({ page }) => {
  const sent = await mockAccount(page);
  await page.goto('/account');
  const language = page.getByRole('combobox', { name: 'Language' });
  await language.focus();
  await page.keyboard.press('Enter');
  // From "This device's language" to "Français".
  await page.keyboard.press('ArrowDown');
  await expect(page.getByRole('option', { name: 'Français' })).toHaveClass(/mat-mdc-option-active/);
  await page.keyboard.press('Enter');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Mon compte');
  await expect(page.getByRole('status')).toHaveText('Préférences enregistrées.');
  expect(sent).toEqual([{ method: 'PUT', path: '/api/v1/me/preferences', body: { locale: 'fr', timeZone: null } }]);
});

test('the password is changed with the current one', async ({ page }) => {
  const sent = await mockAccount(page);
  await page.goto('/account');
  await page.getByRole('button', { name: 'Change', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Change your password' });
  await dialog.getByLabel('Current password').fill('correct horse battery staple');
  await dialog.getByLabel('New password', { exact: true }).fill('a much longer passphrase');
  await dialog.getByLabel('Confirm the new password').fill('a much longer passphrase');
  await dialog.getByRole('button', { name: 'Change the password' }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByRole('status')).toHaveText('Password changed. Your other sessions are signed out.');
  expect(sent[0]).toEqual({
    method: 'POST',
    path: '/api/v1/me/password',
    body: { currentPassword: 'correct horse battery staple', newPassword: 'a much longer passphrase' },
  });
});

test('another device is signed out from the sessions', async ({ page }) => {
  const sent = await mockAccount(page);
  await page.goto('/account');
  await page.getByRole('button', { name: 'Manage' }).click();
  const dialog = page.getByRole('dialog', { name: 'Active sessions' });
  await expect(dialog.getByRole('listitem')).toHaveCount(2);
  await expect(dialog.getByRole('listitem').first()).toContainText('This session');
  await dialog.getByRole('button', { name: 'Sign out the session on Safari · iOS' }).click();
  await expect(dialog.getByRole('status')).toHaveText('Session signed out.');
  await expect(dialog.getByRole('listitem')).toHaveCount(1);
  expect(sent).toEqual([{ method: 'DELETE', path: `/api/v1/me/sessions/${PHONE.id}`, body: null }]);
});

for (const theme of ['light', 'dark'] as const) {
  for (const language of ['fr', 'en'] as const) {
    test.describe(`accessibility of My account, ${theme} mode, ${language}`, () => {
      test.use({ theme, language });

      test('the page and its dialogs have no axe-core violation', async ({ page }) => {
        await mockAccount(page);
        await page.goto('/account');
        await expect(page.getByRole('switch')).toBeChecked();
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'page').toEqual([]);

        await page.locator('button.manage-sessions').click();
        await expect(page.getByRole('dialog').getByRole('listitem')).toHaveCount(2);
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'sessions').toEqual([]);
        await page.keyboard.press('Escape');
        await expect(page.getByRole('dialog')).toHaveCount(0);

        await page.locator('button.change-password').click();
        await expect(page.getByRole('dialog')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'password').toEqual([]);
      });
    });
  }
}
