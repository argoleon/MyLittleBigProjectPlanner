import { SESSION, TENANT, accessibilityViolations, expect, problem, test } from './fixtures';

test.describe('signed out', () => {
  test.use({ signedIn: false });

  test('a protected screen leads to the sign-in page, then back to it', async ({ page }) => {
    let attempts = 0;
    await page.route('**/api/v1/auth/sign-in', async (route) => {
      attempts++;
      const body = route.request().postDataJSON() as { email: string; password: string };
      if (body.password !== 'correct horse battery') {
        return route.fulfill(problem(401, 'auth.invalid_credentials'));
      }
      // Signed in from now on.
      await page.route('**/api/v1/auth/session', (r) => r.fulfill({ json: SESSION }));
      return route.fulfill({ json: SESSION });
    });

    await page.goto('/workload');
    await expect(page).toHaveURL(/\/sign-in\?returnUrl=%2Fworkload$/);
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('Sign in');
    await expect(page.getByText(`Organization: ${TENANT.name}`)).toBeVisible();
    await expect(page).toHaveTitle('Sign in · My Little Big Project Planner');

    await page.getByLabel('Email').fill(SESSION.account.email);
    await page.getByLabel('Password').fill('wrong password');
    await page.getByRole('button', { name: 'Sign in' }).click();
    await expect(page.getByRole('alert')).toHaveText('Incorrect email or password.');
    await expect(page.getByLabel('Password')).toHaveValue('');

    await page.getByLabel('Password').fill('correct horse battery');
    await page.getByLabel('Password').press('Enter');
    await expect(page).toHaveURL(/\/workload$/);
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('Workload');
    expect(attempts).toBe(2);
  });

  test('too many attempts are explained', async ({ page }) => {
    await page.route('**/api/v1/auth/sign-in', (route) => route.fulfill(problem(429, 'too_many_requests')));
    await page.goto('/sign-in');
    await page.getByLabel('Email').fill(SESSION.account.email);
    await page.getByLabel('Password').fill('any password');
    await page.getByRole('button', { name: 'Sign in' }).click();
    await expect(page.getByRole('alert')).toHaveText('Too many attempts. Try again in a few minutes.');
  });

  test('a forgotten password is reset through the emailed link', async ({ page }) => {
    await page.route('**/api/v1/auth/password-reset', (route) => route.fulfill({ status: 202 }));
    await page.route('**/api/v1/auth/password-reset/confirm', (route) => {
      const body = route.request().postDataJSON() as { token: string; password: string };
      return body.token === 'emailed-token' ? route.fulfill({ status: 204 }) : route.fulfill(problem(400, 'auth.reset_token_invalid'));
    });

    await page.goto('/sign-in');
    await page.getByRole('link', { name: 'Forgot your password?' }).click();
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('Forgot your password');
    await page.getByLabel('Email').fill('someone@acme.example');
    await page.getByRole('button', { name: 'Send the link' }).click();
    await expect(page.getByRole('status')).toContainText('If an account matches someone@acme.example');

    await page.goto('/reset-password?token=stale-token');
    await page.getByLabel('New password', { exact: true }).fill('a brand new passphrase');
    await page.getByLabel('Confirm the new password').fill('a brand new passphrase');
    await page.getByRole('button', { name: 'Save the password' }).click();
    await expect(page.getByRole('alert')).toContainText('This link is invalid, has expired or was already used.');
    await expect(page.getByRole('alert').getByRole('link', { name: 'Ask for a new link' })).toBeVisible();

    await page.goto('/reset-password?token=emailed-token');
    await page.getByLabel('New password', { exact: true }).fill('a brand new passphrase');
    await page.getByLabel('Confirm the new password').fill('a brand new passphrase');
    await page.getByRole('button', { name: 'Save the password' }).click();
    await expect(page).toHaveURL(/\/sign-in\?passwordChanged=1$/);
    await expect(page.getByRole('status')).toHaveText('Your password has been changed. Sign in with the new one.');
  });
});

test('the account menu signs out, and a replayed session is refused', async ({ page }) => {
  await page.route('**/api/v1/auth/sign-out', async (route) => {
    await page.route('**/api/v1/auth/session', (r) => r.fulfill(problem(401, 'unauthenticated')));
    return route.fulfill({ status: 204 });
  });
  await page.goto('/gantt');
  await page.getByRole('button', { name: 'Account: Lucas Bernard' }).click();
  await expect(page.getByRole('menu')).toContainText(SESSION.account.email);
  await page.getByRole('menuitem', { name: 'Sign out' }).click();
  await expect(page).toHaveURL(/\/sign-in$/);

  await page.goto('/gantt');
  await expect(page).toHaveURL(/\/sign-in\?returnUrl=%2Fgantt$/);
});

for (const theme of ['light', 'dark'] as const) {
  for (const language of ['fr', 'en'] as const) {
    test.describe(`accessibility of the pages before sign-in, ${theme} mode, ${language}`, () => {
      test.use({ theme, language, signedIn: false });

      test('sign-in, forgotten password and new password have no axe-core violation', async ({ page }) => {
        for (const url of ['/sign-in', '/forgot-password', '/reset-password?token=t']) {
          await page.goto(url);
          await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
          // The validation messages are part of the checked page.
          await page.locator('button[type="submit"]').click();
          await expect(page.locator('mat-error').first()).toBeVisible();
          // Contrast is measured once the messages have faded in.
          await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== 'running'));
          expect(await accessibilityViolations(page), url).toEqual([]);
        }
      });
    });
  }
}
