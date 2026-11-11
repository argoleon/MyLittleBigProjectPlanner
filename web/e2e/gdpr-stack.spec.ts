/**
 * GDPR rights on the production stack, nothing mocked (L02-US09): a member
 * exports their data (the worker collects it into Valkey, the browser
 * downloads it), then erases their account, which signs them out for good.
 *
 * Runs with `just compose-e2e` only: deploy/tests/e2e.sh creates the admin
 * and passes E2E_ADMIN_EMAIL, E2E_ADMIN_SECRET and E2E_MAILPIT_URL.
 */
import { expect, test } from '@playwright/test';

const adminEmail = process.env['E2E_ADMIN_EMAIL'];
const adminSecret = process.env['E2E_ADMIN_SECRET'];
const mailpit = process.env['E2E_MAILPIT_URL'];

test.skip(!adminEmail || !adminSecret || !mailpit, 'needs the compose stack (just compose-e2e)');

test('a member exports their data, then erases their account', async ({ browser, playwright }) => {
  test.setTimeout(120_000);
  const email = `e2e-gdpr-${Date.now()}@example.test`;
  const secret = `gdpr passphrase ${Date.now()}`;

  // The admin invites them, from a browser: it reaches the stack through
  // E2E_HOST_RULES, which Node's requests ignore.
  const adminContext = await browser.newContext();
  const adminPage = await adminContext.newPage();
  await adminPage.goto('/sign-in');
  const invited = await adminPage.evaluate(
    async ({ adminEmail, adminSecret, email }) => {
      const signIn = await fetch('/api/v1/auth/sign-in', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ email: adminEmail, password: adminSecret }),
      });
      const { csrfToken } = (await signIn.json()) as { csrfToken: string };
      const invitation = await fetch('/api/v1/tenant/invitations', {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          'x-csrf-token': csrfToken,
          'idempotency-key': crypto.randomUUID(),
        },
        body: JSON.stringify({ email, accountType: 'internal', tenantRole: 'member' }),
      });
      return invitation.status;
    },
    { adminEmail: adminEmail!, adminSecret: adminSecret!, email },
  );
  expect(invited).toBe(201);
  await adminContext.close();

  // The worker emails the link; they accept it and land in the application.
  const catcher = await playwright.request.newContext({ baseURL: mailpit });
  let token: string | undefined;
  await expect
    .poll(
      async () => {
        const search = await (await catcher.get('/api/v1/search', { params: { query: `to:"${email}"` } })).json();
        const id = search.messages?.[0]?.ID as string | undefined;
        if (!id) return undefined;
        const message = await (await catcher.get(`/api/v1/message/${id}`)).json();
        token = /\/accept-invitation\?token=([A-Za-z0-9_-]+)/.exec(message.Text as string)?.[1];
        return token;
      },
      { timeout: 60_000, message: 'the invitation email never arrived' },
    )
    .toBeTruthy();
  await catcher.dispose();
  const context = await browser.newContext({ acceptDownloads: true });
  await context.addInitScript(() => localStorage.setItem('mlbpp.language', 'en'));
  const page = await context.newPage();
  await page.goto(`/accept-invitation?token=${token}`);
  await page.getByLabel('Your name').fill('E2E Leaving');
  await page.getByLabel('Choose a password').fill(secret);
  await page.getByLabel('Confirm the new password').fill(secret);
  await page.getByRole('button', { name: 'Join' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');

  // They export their data.
  await page.goto('/account');
  const download = page.waitForEvent('download', { timeout: 60_000 });
  await page.getByRole('button', { name: 'Export (JSON)' }).click();
  const file = await download;
  expect(file.suggestedFilename()).toMatch(/^personal-data-\d{4}-\d{2}-\d{2}\.json$/);
  const data = JSON.parse(await (await import('node:fs/promises')).readFile(await file.path(), 'utf8')) as {
    format: string;
    account: { email: string; displayName: string };
    memberships: unknown[];
  };
  expect(data.format).toBe('mlbpp.personal-data.v1');
  expect(data.account).toMatchObject({ email, displayName: 'E2E Leaving' });
  expect(data.memberships).toHaveLength(1);

  // They erase their account: signed out, and gone.
  await page.getByRole('button', { name: 'Delete my account' }).click();
  const dialog = page.getByRole('dialog', { name: 'Delete your account?' });
  await dialog.getByLabel('Your password').fill(secret);
  await dialog.getByLabel(`Type ${email} to confirm`).fill(email);
  await dialog.getByRole('button', { name: 'Delete for good' }).click();
  await expect(page.getByRole('status')).toHaveText('Your account was deleted.');
  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password').fill(secret);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('alert')).toHaveText('Incorrect email or password.');
  await context.close();
});
