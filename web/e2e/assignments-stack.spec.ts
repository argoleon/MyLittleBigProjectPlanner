/**
 * The overload check on the production stack, nothing mocked (L08-US03):
 * a resource already has 4 h on a day through one ticket; the tenant admin
 * assigns it 4 h more on another ticket from the ticket's page. The
 * dialog's preview shows 8 h for 7 h on that day and the assignment is
 * refused as it is; the admin forces it, the ticket flags the overload,
 * and the administration lists it among the forced overloads (L08-US03).
 * A leave declared on those hours then shows them to reschedule, on the
 * absence and on the ticket (L08-US04). The resource's Assignments tab
 * lists both stories under their project (L08-US05).
 *
 * Runs with `just compose-e2e` only: deploy/tests/e2e.sh creates the admin
 * and passes E2E_ADMIN_EMAIL and E2E_ADMIN_SECRET.
 */
import { type Locator, type Page, expect, test } from '@playwright/test';

import { accessibilityViolations } from './fixtures';

const adminEmail = process.env['E2E_ADMIN_EMAIL'];
const adminSecret = process.env['E2E_ADMIN_SECRET'];

test.skip(!adminEmail || !adminSecret, 'needs the compose stack (just compose-e2e)');

const DAY = 86_400_000;

/** As the date picker reads it in English. */
const typed = (time: number) => {
  const date = new Date(time);
  return `${date.getUTCMonth() + 1}/${date.getUTCDate()}/${date.getUTCFullYear()}`;
};

/**
 * A Wednesday of next month that is never a public holiday: the first one,
 * unless it is the 1st (then a week later, two in May for the 8th).
 */
function plainWednesday(): number {
  const now = new Date();
  const first = Date.UTC(now.getFullYear(), now.getMonth() + 1, 1);
  const weekday = (new Date(first).getUTCDay() + 6) % 7;
  let wednesday = first + ((2 - weekday + 7) % 7) * DAY;
  const date = new Date(wednesday);
  if (date.getUTCDate() === 1) wednesday += (date.getUTCMonth() === 4 ? 14 : 7) * DAY;
  return wednesday;
}

async function opened(dialog: Locator): Promise<void> {
  await expect(dialog.locator(':focus')).toHaveCount(1);
}

/**
 * Through the API, as the admin signed in on `page`: blocking mode without
 * tolerance, the project `key` with two stories, a resource `name` and 4 h
 * of it on `day` through the second story; the resource's id.
 */
async function seed(page: Page, key: string, name: string, day: string): Promise<string> {
  return page.evaluate(
    async ({ key, name, day }) => {
      const session = (await (await fetch('/api/v1/auth/session')).json()) as { csrfToken: string };
      const headers = { 'content-type': 'application/json', 'x-csrf-token': session.csrfToken };
      const call = async (path: string, method: string, body: object, extra: Record<string, string> = {}) => {
        const response = await fetch(path, { method, headers: { ...headers, ...extra }, body: JSON.stringify(body) });
        if (response.status >= 300) throw new Error(`${method} ${path}: ${response.status} ${await response.text()}`);
        return response.json();
      };
      const settings = (await (await fetch('/api/v1/tenant/workload-settings')).json()) as { defaultCapacityMinutes: number[] };
      await call(
        '/api/v1/tenant/workload-settings',
        'PUT',
        { defaultCapacityMinutes: settings.defaultCapacityMinutes, overloadTolerancePercent: 0, overloadMode: 'blocking' },
        { 'if-match': '*' },
      );
      await call('/api/v1/nodes', 'POST', { type: 'project', name: `Overload ${key}`, key });
      const form = (await (await fetch(`/api/v1/projects/${key}/issue-form`)).json()) as { types: { id: string; key: string }[] };
      const story = form.types.find((type) => type.key === 'story');
      for (const summary of ['Account migration scripts', 'Directory sync']) {
        await call(`/api/v1/projects/${key}/issues`, 'POST', { summary, typeId: story?.id, priority: 'medium' }, { 'idempotency-key': crypto.randomUUID() });
      }
      const resource = (await call('/api/v1/resources', 'POST', { kind: 'generic', displayName: name })) as { id: string };
      // A 7 h day by default: keep it so whatever the tenant's default.
      await call(`/api/v1/resources/${resource.id}/capacity-patterns`, 'POST', { effectiveFrom: '2000-01-03', minutes: [420, 420, 420, 420, 420, 0, 0] });
      await call(`/api/v1/issues/${key}-2/assignments`, 'POST', {
        resourceId: resource.id,
        segments: [{ start: day, end: day, mode: 'hours_per_day', value: 240 }],
      });
      return resource.id;
    },
    { key, name, day },
  );
}

test('an overloading assignment is shown day by day, refused, then forced and traced', async ({ page }) => {
  test.setTimeout(120_000);
  const stamp = Date.now();
  const key = `OV${String(stamp).slice(-6)}`;
  const name = `E2E Overload ${stamp}`;
  const wednesday = plainWednesday();
  const day = new Date(wednesday).toISOString().slice(0, 10);

  await page.context().addInitScript(() => localStorage.setItem('mlbpp.language', 'en'));
  await page.goto('/sign-in');
  await page.getByLabel('Email').fill(adminEmail!);
  await page.getByLabel('Password').fill(adminSecret!);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');
  const resource = await seed(page, key, name, day);

  // 4 h more on that Wednesday, from the first story's page.
  await page.goto(`/issues/${key}-1`);
  const card = page.locator('app-issue-assignments');
  await card.getByRole('button', { name: 'Add a resource' }).click();
  const dialog = page.getByRole('dialog');
  await opened(dialog);
  await dialog.getByRole('combobox', { name: 'Resource' }).fill(name);
  await page.getByRole('option', { name }).click();
  await dialog.getByLabel('Start of period 1').fill(typed(wednesday));
  await dialog.getByLabel('End of period 1').fill(typed(wednesday));
  await dialog.getByLabel('Value 1').fill('4');

  // The preview: 4 h existing, 4 h of this assignment, 8 h for 7 h.
  const preview = dialog.locator('.preview-table');
  await expect(preview.locator('tbody tr').nth(3).locator('td')).toHaveText(['8 h warningoverloaded']);
  await expect(preview.locator('tbody tr').nth(1).locator('td')).toHaveText(['4 h']);
  await expect(dialog.getByRole('alert')).toContainText('8 h planned for 7 h available (+1 h)');
  await expect(dialog.getByRole('button', { name: 'Save' })).toBeDisabled();
  expect(await accessibilityViolations(page), 'assignment dialog with an overload').toEqual([]);

  // Forced, written and flagged on the ticket.
  await dialog.getByRole('button', { name: 'Force overload' }).click();
  await expect(dialog).toHaveCount(0);
  await expect(card.getByRole('status')).toHaveText(`${name} was assigned to the ticket.`);
  const block = card.locator('article.assignment', { hasText: name });
  await expect(block.locator('.tag.overload')).toHaveText('warning Overload');
  await expect(block.getByRole('note')).toContainText('Resource overloaded');

  // Traced among the forced overloads.
  await page.goto('/admin/forced-overloads');
  const row = page.locator('tr.mat-mdc-row', { hasText: `${key}-1` });
  await expect(row).toContainText(name);
  await expect(row).toContainText('8 h planned for 7 h available (+1 h)');
  expect(await accessibilityViolations(page), 'forced overloads').toEqual([]);

  // The resource's Assignments tab: both stories, under their project (L08-US05).
  await page.goto(`/workload/resources/${resource}`);
  await page.getByRole('tab', { name: 'Assignments' }).click();
  const tab = page.locator('app-resource-assignments');
  const group = tab.locator('section.group', { hasText: `Overload ${key}` });
  await expect(group.locator('.key')).toHaveText([`${key}-1`, `${key}-2`]);
  await expect(group.locator('.group-load')).toHaveText('8 h');
  await expect(group.locator('.tag.overload')).toHaveCount(2);
  expect(await accessibilityViolations(page), 'resource assignments').toEqual([]);
});

test('a leave on planned hours shows them to reschedule on the absence and the ticket', async ({ page }) => {
  test.setTimeout(120_000);
  const stamp = Date.now();
  const key = `CF${String(stamp).slice(-6)}`;
  const name = `E2E Conflict ${stamp}`;
  const wednesday = plainWednesday();
  const day = new Date(wednesday).toISOString().slice(0, 10);

  await page.context().addInitScript(() => localStorage.setItem('mlbpp.language', 'en'));
  await page.goto('/sign-in');
  await page.getByLabel('Email').fill(adminEmail!);
  await page.getByLabel('Password').fill(adminSecret!);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');
  const resource = await seed(page, key, name, day);

  // A day of leave on the 4 h planned through the second story.
  await page.goto(`/workload/resources/${resource}`);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(name);
  await page.getByRole('button', { name: 'Declare an absence' }).first().click();
  const dialog = page.getByRole('dialog');
  await opened(dialog);
  await dialog.getByRole('combobox', { name: 'Kind' }).click();
  await page.getByRole('option', { name: 'Leave' }).click();
  await dialog.getByLabel('First day').fill(typed(wednesday));
  await dialog.getByRole('button', { name: 'Declare' }).click();
  const absences = page.locator('app-absences-card');
  await expect(absences.getByRole('status')).toHaveText('The absence was declared.');

  // The worker finds the conflict within seconds.
  await expect(async () => {
    await page.reload();
    await expect(page.locator('app-absences-card .conflict')).toHaveText('event_busy 4 h in conflict', { timeout: 2_000 });
  }).toPass({ timeout: 30_000 });
  expect(await accessibilityViolations(page), 'resource page with a conflict').toEqual([]);

  // On the ticket: the day and its hours.
  await page.goto(`/issues/${key}-2`);
  const block = page.locator('app-issue-assignments article.assignment', { hasText: name });
  await expect(block.locator('.conflicts-title')).toHaveText('4 h to reschedule: the resource is away');
  await expect(block.locator('.conflicts-days li')).toHaveCount(1);
  await expect(block.locator('.conflicts-days li')).toContainText(': 4 h');
});
