/**
 * Inviting and removing a member on the production stack, nothing mocked
 * (L02-US06): the tenant admin invites, the worker emails the link (read
 * back from Mailpit), the invitee chooses a password and lands in the
 * application; the admin removes them and their access ends at once.
 *
 * Runs with `just compose-e2e` only: deploy/tests/e2e.sh creates the admin
 * and passes E2E_ADMIN_EMAIL, E2E_ADMIN_SECRET and E2E_MAILPIT_URL.
 */
import { expect, test } from '@playwright/test';

import { accessibilityViolations } from './fixtures';

const adminEmail = process.env['E2E_ADMIN_EMAIL'];
const adminSecret = process.env['E2E_ADMIN_SECRET'];
const mailpit = process.env['E2E_MAILPIT_URL'];

test.skip(!adminEmail || !adminSecret || !mailpit, 'needs the compose stack (just compose-e2e)');

test('invite a member, who joins, then remove them', async ({ browser, playwright }) => {
  test.setTimeout(120_000);
  const invitee = `e2e-invitee-${Date.now()}@example.test`;
  const inviteeSecret = `invitee passphrase ${Date.now()}`;

  // The admin invites.
  const adminContext = await browser.newContext();
  await adminContext.addInitScript(() => localStorage.setItem('mlbpp.language', 'en'));
  const admin = await adminContext.newPage();
  await admin.goto('/sign-in');
  await admin.getByLabel('Email').fill(adminEmail!);
  await admin.getByLabel('Password').fill(adminSecret!);
  await admin.getByRole('button', { name: 'Sign in' }).click();
  await expect(admin.getByRole('heading', { level: 1 })).toHaveText('Home');
  await admin.goto('/admin/members');
  await expect(admin.getByRole('tab', { name: 'Members' })).toHaveAttribute('aria-selected', 'true');
  await expect(admin.getByRole('row').first()).toBeVisible();
  expect(await accessibilityViolations(admin), 'members tab').toEqual([]);
  await admin.getByRole('button', { name: 'Invite' }).click();
  const dialog = admin.getByRole('dialog');
  await dialog.getByLabel('Email').fill(invitee);
  await dialog.getByRole('button', { name: 'Send the invitation' }).click();
  await expect(admin.getByRole('status')).toHaveText(`Invitation sent to ${invitee}.`);
  await expect(admin.getByRole('row').filter({ hasText: invitee })).toContainText('Invitation sent');

  // The worker emails the link.
  const catcher = await playwright.request.newContext({ baseURL: mailpit });
  let token: string | undefined;
  await expect
    .poll(
      async () => {
        const search = await (await catcher.get('/api/v1/search', { params: { query: `to:"${invitee}"` } })).json();
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

  // The invitee joins, in a browser of their own.
  const inviteeContext = await browser.newContext();
  await inviteeContext.addInitScript(() => localStorage.setItem('mlbpp.language', 'en'));
  const page = await inviteeContext.newPage();
  await page.goto(`/accept-invitation?token=${token}`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Join an organization');
  await expect(page.getByText('E2E Admin invites you to join')).toBeVisible();
  expect(await accessibilityViolations(page), 'invitation page').toEqual([]);
  await page.getByLabel('Your name').fill('E2E Invitee');
  await page.getByLabel('Choose a password').fill(inviteeSecret);
  await page.getByLabel('Confirm the new password').fill(inviteeSecret);
  await page.getByRole('button', { name: 'Join' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');
  await expect(page.getByRole('button', { name: 'Account: E2E Invitee' })).toBeVisible();

  // The admin sees the new member, then removes them.
  await admin.reload();
  const row = admin.getByRole('row').filter({ hasText: invitee });
  await expect(row).toContainText('Active');
  await row.getByRole('button', { name: /^Actions for / }).click();
  await admin.getByRole('menuitem', { name: 'Remove from the organization' }).click();
  await admin.getByRole('dialog').getByRole('button', { name: 'Remove' }).click();
  await expect(admin.getByRole('status')).toHaveText('E2E Invitee was removed from the organization.');
  await expect(admin.getByRole('row').filter({ hasText: invitee })).toHaveCount(0);

  // The invitee's next request is refused: back to the sign-in page.
  await page.goto('/workload');
  await expect(page).toHaveURL(/\/sign-in/);
  await page.getByLabel('Email').fill(invitee);
  await page.getByLabel('Password').fill(inviteeSecret);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('alert')).toHaveText('Incorrect email or password.');

  await inviteeContext.close();
  await adminContext.close();
});
