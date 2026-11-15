/**
 * Notifications on the production stack, nothing mocked (L05-US07): the
 * admin gives a member a role on a project and mentions them in a comment
 * on a ticket; the worker notifies them within seconds; the member sees it
 * on the bell, opens it, lands on the ticket; their preferences are kept.
 *
 * Runs with `just compose-e2e` only: deploy/tests/e2e.sh creates the admin
 * and the member, and passes E2E_ADMIN_EMAIL, E2E_ADMIN_SECRET,
 * E2E_NOTIFIED_EMAIL and E2E_NOTIFIED_SECRET.
 */
import { type Browser, type Page, expect, test } from '@playwright/test';

import { accessibilityViolations } from './fixtures';

const adminEmail = process.env['E2E_ADMIN_EMAIL'];
const adminSecret = process.env['E2E_ADMIN_SECRET'];
const memberEmail = process.env['E2E_NOTIFIED_EMAIL'];
const memberSecret = process.env['E2E_NOTIFIED_SECRET'];

test.skip(!adminEmail || !adminSecret || !memberEmail || !memberSecret, 'needs the compose stack (just compose-e2e)');

/** A browser of their own for the member `email`, signed in, in English. */
async function signIn(browser: Browser, email: string, secret: string): Promise<Page> {
  const context = await browser.newContext();
  await context.addInitScript(() => localStorage.setItem('mlbpp.language', 'en'));
  const page = await context.newPage();
  await page.goto('/sign-in');
  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password').fill(secret);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');
  return page;
}

test('a mention reaches the member on the bell, and opens the ticket', async ({ browser }) => {
  test.setTimeout(180_000);
  const run = Date.now().toString(36).toUpperCase().slice(-5);
  const code = `NT${run}`.slice(0, 10);
  const name = `Notify ${run}`;
  const admin = await signIn(browser, adminEmail!, adminSecret!);

  // A project the member may read, and a story in it.
  await admin.goto('/portfolios');
  await admin.getByRole('button', { name: 'New node' }).click();
  const dialog = admin.getByRole('dialog');
  await dialog.getByRole('combobox', { name: 'Type' }).click();
  await admin.getByRole('option', { name: 'Project', exact: true }).click();
  await dialog.getByLabel('Name').fill(name);
  await dialog.getByLabel('Key').fill(code);
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(admin.getByRole('heading', { level: 1 })).toHaveText(`${name} · Settings`);
  const projectUrl = admin.url().replace(/\/general$/, '');
  await admin.getByRole('tab', { name: 'Attachments and permissions' }).click();
  const access = admin.getByRole('region', { name: 'Effective access' });
  await access.getByRole('button', { name: 'Add a role' }).click();
  const grant = admin.getByRole('dialog', { name: `Grant a role on ${name}` });
  await grant.getByLabel('Member or group').fill(memberEmail!);
  await admin.getByRole('option', { name: memberEmail! }).click();
  await grant.getByRole('combobox', { name: 'Role' }).click();
  await admin.getByRole('option', { name: 'Reader', exact: true }).click();
  await grant.getByRole('button', { name: 'Grant' }).click();
  await expect(access.getByRole('status')).toHaveText('Role Reader granted to E2E Notified.');
  await admin.goto(projectUrl);
  await admin.getByRole('button', { name: 'New ticket' }).click();
  await dialog.getByRole('combobox', { name: 'Type' }).click();
  await admin.getByRole('option', { name: 'Story', exact: true }).click();
  await dialog.getByLabel('Summary').fill(`Review ${run}`);
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(admin).toHaveURL(new RegExp(`/issues/${code}-1$`));
  await expect(admin.getByRole('button', { name: 'Watching (1)' })).toHaveAttribute('aria-pressed', 'true');

  // The admin mentions the member.
  await admin.getByRole('tab', { name: 'Comments (0)' }).click();
  const comments = admin.locator('app-issue-comments');
  await comments.getByRole('textbox', { name: 'Add a comment' }).click();
  // By the start of their email: every run of the stack adds an "E2E Notified".
  await admin.keyboard.type(`@${memberEmail!.split('@')[0]}`);
  await expect(admin.getByRole('option')).toHaveCount(1);
  await admin.getByRole('option', { name: 'E2E Notified' }).click();
  await admin.keyboard.type(' please review');
  await comments.getByRole('button', { name: 'Comment', exact: true }).click();
  await expect(comments.locator('.comment')).toHaveCount(1);

  // The member is told on the bell, within seconds.
  const member = await signIn(browser, memberEmail!, memberSecret!);
  await expect(async () => {
    await member.goto('/');
    await expect(member.getByRole('link', { name: /^Notifications, \d+ unread$/ })).toBeVisible({ timeout: 2_000 });
  }).toPass({ timeout: 60_000 });
  await member.getByRole('link', { name: /^Notifications, \d+ unread$/ }).click();
  const item = member.locator('.item', { hasText: `${code}-1` });
  await expect(item.locator('.what')).toHaveText(`E2E Admin mentioned you on ${code}-1 (unread)`);
  await expect(item.locator('.detail')).toContainText('please review');
  expect(await accessibilityViolations(member)).toEqual([]);
  await item.click();
  await expect(member).toHaveURL(new RegExp(`/issues/${code}-1$`));
  await expect(member.getByRole('heading', { level: 1 })).toHaveText(`Review ${run}`);

  // The member's preferences are kept.
  await member.goto('/notifications');
  const comment = member.getByRole('checkbox', { name: 'New comment by email' });
  const wanted = !(await comment.isChecked());
  const saved = member.waitForResponse(
    (response) => response.url().endsWith('/notification-preferences') && response.request().method() === 'PUT',
  );
  await comment.setChecked(wanted);
  expect((await saved).ok()).toBe(true);
  await member.reload();
  await expect(member.getByRole('checkbox', { name: 'New comment by email' })).toBeChecked({ checked: wanted });
});
