/**
 * The whole sign-in flow on the production stack, nothing mocked: real API,
 * sessions in Valkey, the reset email sent by the worker and read back from
 * the SMTP catcher (Mailpit).
 *
 * Runs with `just compose-e2e` only: deploy/tests/e2e.sh creates the member
 * and passes E2E_MEMBER_EMAIL, E2E_MEMBER_SECRET and E2E_MAILPIT_URL.
 */
import { expect, test } from '@playwright/test';

import { accessibilityViolations } from './fixtures';

const email = process.env['E2E_MEMBER_EMAIL'];
const secret = process.env['E2E_MEMBER_SECRET'];
const mailpit = process.env['E2E_MAILPIT_URL'];

test.skip(!email || !secret || !mailpit, 'needs the compose stack (just compose-e2e)');

test('sign in, sign out, reset the password from the email, sign in again', async ({ page, playwright }) => {
  test.setTimeout(120_000);
  const newSecret = `${secret} renewed`;
  await page.addInitScript(() => localStorage.setItem('mlbpp.language', 'en'));

  const signIn = async (password: string) => {
    await page.getByLabel('Email').fill(email!);
    await page.getByLabel('Password').fill(password);
    await page.getByRole('button', { name: 'Sign in' }).click();
  };

  // Sign in, reach the shell.
  await page.goto('/');
  await expect(page).toHaveURL(/\/sign-in$/);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Sign in');
  expect(await accessibilityViolations(page), 'sign-in page').toEqual([]);
  await signIn(secret!);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');
  await expect(page.getByRole('button', { name: /^Account: / })).toBeVisible();
  expect(await accessibilityViolations(page), 'home').toEqual([]);

  // Sign out.
  await page.getByRole('button', { name: /^Account: / }).click();
  await page.getByRole('menuitem', { name: 'Sign out' }).click();
  await expect(page).toHaveURL(/\/sign-in$/);
  await page.goto('/workload');
  await expect(page).toHaveURL(/\/sign-in\?returnUrl=%2Fworkload$/);

  // Ask for a reset link.
  await page.getByRole('link', { name: 'Forgot your password?' }).click();
  await page.getByLabel('Email').fill(email!);
  await page.getByRole('button', { name: 'Send the link' }).click();
  await expect(page.getByRole('status')).toContainText(`If an account matches ${email}`);
  expect(await accessibilityViolations(page), 'forgot-password page').toEqual([]);

  // Read the email the worker sent.
  const catcher = await playwright.request.newContext({ baseURL: mailpit });
  let token: string | undefined;
  await expect
    .poll(
      async () => {
        const search = await (await catcher.get('/api/v1/search', { params: { query: `to:"${email}"` } })).json();
        const id = search.messages?.[0]?.ID as string | undefined;
        if (!id) return undefined;
        const message = await (await catcher.get(`/api/v1/message/${id}`)).json();
        token = /\/reset-password\?token=([A-Za-z0-9_-]+)/.exec(message.Text as string)?.[1];
        return token;
      },
      { timeout: 60_000, message: 'the reset email never arrived' },
    )
    .toBeTruthy();
  await catcher.dispose();

  // Choose a new password with the link.
  await page.goto(`/reset-password?token=${token}`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Choose a new password');
  expect(await accessibilityViolations(page), 'reset-password page').toEqual([]);
  await page.getByLabel('New password', { exact: true }).fill(newSecret);
  await page.getByLabel('Confirm the new password').fill(newSecret);
  await page.getByRole('button', { name: 'Save the password' }).click();
  await expect(page).toHaveURL(/\/sign-in\?passwordChanged=1$/);
  await expect(page.getByRole('status')).toHaveText('Your password has been changed. Sign in with the new one.');

  // The old password no longer works; the new one does.
  await signIn(secret!);
  await expect(page.getByRole('alert')).toHaveText('Incorrect email or password.');
  await signIn(newSecret);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');

  // The link worked once.
  await page.goto(`/reset-password?token=${token}`);
  await page.getByLabel('New password', { exact: true }).fill('yet another passphrase');
  await page.getByLabel('Confirm the new password').fill('yet another passphrase');
  await page.getByRole('button', { name: 'Save the password' }).click();
  await expect(page.getByRole('alert')).toContainText('This link is invalid, has expired or was already used.');
});
