import type { Page } from '@playwright/test';

import { SESSION, accessibilityViolations, expect, problem, test } from './fixtures';
import { base32Decode, totp } from './totp';

const SETUP = {
  otpauthUri: 'otpauth://totp/My%20Little%20Big%20Project%20Planner:lucas?secret=JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP',
  secret: 'JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP',
  qrCodeSvg:
    '<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200"><rect width="200" height="200" fill="#fff"/><rect x="20" y="20" width="60" height="60"/></svg>',
};
const CODES = ['abcde-fghjk', 'mnpqr-stuvw', 'xyz23-45678', 'abcde-23456', 'fghjk-mnpqr', 'stuvw-xyz23', 'hjkmn-pqrst', 'uvwxy-z2345'];

const at = (stage: string) => ({ ...SESSION, stage });

/** The API of a member whose password was right, at `stage`. */
async function afterPassword(page: Page, stage: 'mfa_challenge' | 'mfa_enrollment') {
  let current = at(stage);
  await page.route('**/api/v1/auth/session', (route) => route.fulfill({ json: current }));
  await page.route('**/api/v1/auth/sign-in/verify', (route) => {
    const { code } = route.request().postDataJSON() as { code: string };
    if (code !== '482913' && code !== 'abcde-fghjk') return route.fulfill(problem(401, 'auth.invalid_code'));
    current = at('complete');
    return route.fulfill({ json: current });
  });
  await page.route('**/api/v1/account/mfa/setup', (route) => route.fulfill({ json: SETUP }));
  await page.route('**/api/v1/account/mfa/confirm', (route) => {
    current = at('complete');
    return route.fulfill({ json: { backupCodes: CODES, session: current } });
  });
}

test('the TOTP helper of the tests matches RFC 6238', () => {
  // Appendix B: secret "12345678901234567890", T = 59 s, 6 digits.
  const secret = 'GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ'; // gitleaks:allow (public RFC 6238 test vector)
  expect(base32Decode(secret).toString()).toBe('12345678901234567890');
  expect(totp(secret, 0, 59_000)).toBe('287082');
});

test('after the password, a code from the app opens the application', async ({ page }) => {
  await afterPassword(page, 'mfa_challenge');
  await page.goto('/workload');
  await expect(page).toHaveURL(/\/sign-in\/verify\?returnUrl=%2Fworkload$/);
  await expect(page.getByText('Step 2 of 2')).toBeVisible();

  await page.getByLabel('6-digit code').fill('000000');
  await page.getByRole('button', { name: 'Verify' }).click();
  await expect(page.getByRole('alert')).toHaveText('Incorrect or already used code.');

  await page.getByRole('button', { name: 'Use a backup code' }).click();
  await page.getByLabel('Backup code').fill('abcde-fghjk');
  await page.getByRole('button', { name: 'Verify' }).click();
  await expect(page).toHaveURL(/\/workload$/);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Workload');
});

test('a member the organization requires to enroll does so before anything else', async ({ page }) => {
  await afterPassword(page, 'mfa_enrollment');
  await page.goto('/gantt');
  await expect(page).toHaveURL(/\/sign-in\/enroll\?returnUrl=%2Fgantt$/);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Enable two-step verification');
  await expect(page.getByRole('img', { name: /QR code/ })).toBeVisible();
  await expect(page.locator('code.key')).toHaveText('JBSW Y3DP EHPK 3PXP JBSW Y3DP EHPK 3PXP');

  await page.getByLabel('6-digit code').fill('482913');
  await page.getByRole('button', { name: 'Enable' }).click();
  await expect(page.getByRole('status')).toHaveText('Two-step verification is enabled.');
  await expect(page.getByRole('list', { name: 'Backup codes' }).getByRole('listitem')).toHaveCount(8);
  await page.getByRole('button', { name: 'I kept my codes' }).click();
  await expect(page).toHaveURL(/\/gantt$/);
});

test('the account menu opens two-step verification', async ({ page }) => {
  let enabled = false;
  await page.route('**/api/v1/account/mfa', (route) =>
    route.fulfill({ json: { enabled, backupCodesLeft: enabled ? 8 : 0, requiredByPolicy: false } }),
  );
  await page.route('**/api/v1/account/mfa/setup', (route) => route.fulfill({ json: SETUP }));
  await page.route('**/api/v1/account/mfa/confirm', (route) => {
    enabled = true;
    return route.fulfill({ json: { backupCodes: CODES, session: SESSION } });
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Account: Lucas Bernard' }).click();
  await page.getByRole('menuitem', { name: 'Two-step verification' }).click();
  const dialog = page.getByRole('dialog', { name: 'Two-step verification' });
  await expect(dialog.getByRole('img', { name: /QR code/ })).toBeVisible();
  await dialog.getByLabel('6-digit code').fill('482913');
  await dialog.getByRole('button', { name: 'Enable' }).click();
  await expect(dialog.getByRole('listitem')).toHaveCount(8);
  await dialog.getByRole('button', { name: 'I kept my codes' }).click();
  await expect(dialog).toBeHidden();

  await page.getByRole('button', { name: 'Account: Lucas Bernard' }).click();
  await page.getByRole('menuitem', { name: 'Two-step verification' }).click();
  await expect(page.getByRole('dialog').getByRole('status')).toHaveText(
    'Two-step verification is enabled. Backup codes left: 8.',
  );
});

/** Lets the fade-in animations end before measuring contrast. */
async function animationsDone(page: Page) {
  await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== 'running'));
}

for (const theme of ['light', 'dark'] as const) {
  for (const language of ['fr', 'en'] as const) {
    test.describe(`accessibility of two-step verification, ${theme} mode, ${language}`, () => {
      test.use({ theme, language });

      test('the code step, the enrollment and the dialog have no axe-core violation', async ({ page }) => {
        await afterPassword(page, 'mfa_challenge');
        await page.goto('/sign-in/verify');
        await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
        await page.locator('button[type="submit"]').click();
        await expect(page.locator('mat-error')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'code step').toEqual([]);

        await page.unrouteAll({ behavior: 'ignoreErrors' });
        await afterPassword(page, 'mfa_enrollment');
        await page.goto('/sign-in/enroll');
        await expect(page.locator('img.qr')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'enrollment').toEqual([]);
        await page.locator('input[formcontrolname="code"]').fill('482913');
        await page.locator('form button[type="submit"]').click();
        await expect(page.locator('ol.codes li')).toHaveCount(8);
        await animationsDone(page);
        expect(await accessibilityViolations(page), 'backup codes').toEqual([]);
      });
    });
  }
}
