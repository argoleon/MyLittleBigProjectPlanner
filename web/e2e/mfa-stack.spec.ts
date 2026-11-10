/**
 * Two-step verification on the production stack, nothing mocked (L02-US03):
 * enable it with a TOTP code computed here, sign out, sign in with a code,
 * then with a backup code, which then works no more.
 *
 * Runs with `just compose-e2e` only: deploy/tests/e2e.sh creates the member
 * and passes E2E_MFA_EMAIL and E2E_MFA_SECRET.
 */
import { type Page, expect, test } from '@playwright/test';

import { accessibilityViolations } from './fixtures';
import { totp } from './totp';

const email = process.env['E2E_MFA_EMAIL'];
const password = process.env['E2E_MFA_SECRET'];

test.skip(!email || !password, 'needs the compose stack (just compose-e2e)');

async function animationsDone(page: Page) {
  await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== 'running'));
}

async function signInWithPassword(page: Page) {
  await page.goto('/sign-in');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Sign in');
  await page.getByLabel('Email').fill(email!);
  await page.getByLabel('Password').fill(password!);
  await page.getByRole('button', { name: 'Sign in' }).click();
}

async function signOut(page: Page) {
  await page.getByRole('button', { name: /^Account: / }).click();
  await page.getByRole('menuitem', { name: 'Sign out' }).click();
  await expect(page).toHaveURL(/\/sign-in$/);
}

test('enable two-step verification, then sign in with a code and with a backup code', async ({ page }) => {
  test.setTimeout(120_000);
  await page.addInitScript(() => localStorage.setItem('mlbpp.language', 'en'));

  // Signed in with the password alone: enable it from the account menu.
  await signInWithPassword(page);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');
  await page.getByRole('button', { name: /^Account: / }).click();
  await page.getByRole('menuitem', { name: 'Two-step verification' }).click();
  const dialog = page.getByRole('dialog', { name: 'Two-step verification' });
  await expect(dialog.getByRole('img', { name: /QR code/ })).toBeVisible();
  await animationsDone(page);
  expect(await accessibilityViolations(page), 'enrollment dialog').toEqual([]);
  const secret = (await dialog.locator('code.key').textContent())!.replace(/\s/g, '');
  const enrollmentCode = totp(secret);
  await dialog.getByLabel('6-digit code').fill(enrollmentCode);
  await dialog.getByRole('button', { name: 'Enable' }).click();
  const codes = dialog.getByRole('list', { name: 'Backup codes' }).getByRole('listitem');
  await expect(codes).toHaveCount(8);
  const backupCodes = (await codes.allTextContents()).map((c) => c.trim());
  await animationsDone(page);
  expect(await accessibilityViolations(page), 'backup codes').toEqual([]);
  await dialog.getByRole('button', { name: 'I kept my codes' }).click();
  await expect(dialog).toBeHidden();
  await signOut(page);

  // The password alone is not enough any more: a code from the app.
  await signInWithPassword(page);
  await expect(page).toHaveURL(/\/sign-in\/verify$/);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Two-step verification');
  expect(await accessibilityViolations(page), 'code step').toEqual([]);
  await page.goto('/workload');
  await expect(page).toHaveURL(/\/sign-in\/verify\?returnUrl=%2Fworkload$/, { timeout: 10_000 });
  // The enrollment used the current step: the app's next code.
  let code = totp(secret, 1);
  if (code === enrollmentCode) code = totp(secret, 2);
  await page.getByLabel('6-digit code').fill(code);
  await page.getByRole('button', { name: 'Verify' }).click();
  await expect(page).toHaveURL(/\/workload$/);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Workload');
  await signOut(page);

  // A backup code, once.
  await signInWithPassword(page);
  await expect(page).toHaveURL(/\/sign-in\/verify$/);
  await page.getByRole('button', { name: 'Use a backup code' }).click();
  await page.getByLabel('Backup code').fill(backupCodes[0]);
  await page.getByRole('button', { name: 'Verify' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');
  await signOut(page);

  await signInWithPassword(page);
  await expect(page).toHaveURL(/\/sign-in\/verify$/);
  await page.getByRole('button', { name: 'Use a backup code' }).click();
  await page.getByLabel('Backup code').fill(backupCodes[0]);
  await page.getByRole('button', { name: 'Verify' }).click();
  await expect(page.getByRole('alert')).toHaveText('Incorrect or already used code.');
});
