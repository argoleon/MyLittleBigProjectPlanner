/**
 * The capacity of a resource from end to end on the production stack,
 * nothing mocked (L07-US06): the admin creates a person on a Paris site,
 * gives it a part-time week (Wednesday off, a Friday of 3.5 h), declares a
 * day of leave and a half day, then reads the hours left in the month grid,
 * the month's total and the next 4 weeks.
 *
 * The site has no public holidays, so that the expected hours do not
 * depend on the month the test runs in. Runs with `just compose-e2e` only:
 * deploy/tests/e2e.sh creates the admin and passes E2E_ADMIN_EMAIL and
 * E2E_ADMIN_SECRET.
 */
import { expect, test } from '@playwright/test';

import { accessibilityViolations } from './fixtures';

const adminEmail = process.env['E2E_ADMIN_EMAIL'];
const adminSecret = process.env['E2E_ADMIN_SECRET'];

test.skip(!adminEmail || !adminSecret, 'needs the compose stack (just compose-e2e)');

/** Minutes per weekday, Monday first: Wednesday off, a short Friday. */
const WEEK = [420, 420, 0, 420, 210, 0, 0];
const DAY = 86_400_000;

/** The local date of a UTC midnight. */
const iso = (time: number) => new Date(time).toISOString().slice(0, 10);
const weekday = (time: number) => (new Date(time).getUTCDay() + 6) % 7;
/** As the date picker reads it in English. */
const typed = (time: number) => {
  const date = new Date(time);
  return `${date.getUTCMonth() + 1}/${date.getUTCDate()}/${date.getUTCFullYear()}`;
};
/** As the interface writes a duration in English. */
const hours = (minutes: number) => {
  const rest = minutes % 60;
  const whole = `${Math.trunc(minutes / 60).toLocaleString('en')} h`;
  return rest === 0 ? whole : `${whole} ${String(rest).padStart(2, '0')} min`;
};

test('the capacity of a part-time person after a leave and a half day', async ({ browser }) => {
  test.setTimeout(150_000);
  const stamp = Date.now();
  const site = `E2E Paris ${stamp}`;
  const name = `E2E Capacity flow ${stamp}`;

  // The first Thursday of next month, a day of leave, and the afternoon of the Friday after it.
  const now = new Date();
  const today = Date.UTC(now.getFullYear(), now.getMonth(), now.getDate());
  const nextMonth = Date.UTC(now.getFullYear(), now.getMonth() + 1, 1);
  const thursday = nextMonth + ((3 - weekday(nextMonth) + 7) % 7) * DAY;
  const friday = thursday + DAY;
  const minutesOn = (time: number) => (time === thursday ? 0 : time === friday ? 105 : WEEK[weekday(time)]);

  const context = await browser.newContext();
  await context.addInitScript(() => localStorage.setItem('mlbpp.language', 'en'));
  const page = await context.newPage();
  await page.goto('/sign-in');
  await page.getByLabel('Email').fill(adminEmail!);
  await page.getByLabel('Password').fill(adminSecret!);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Home');

  // The site, without public holidays.
  await page.goto('/workload/calendars');
  await page.getByRole('button', { name: 'New site' }).click();
  let dialog = page.getByRole('dialog');
  await dialog.getByLabel('Name (English)').fill(site);
  await dialog.getByLabel('Name (Français)').fill(site);
  await dialog.getByRole('combobox', { name: 'Public holidays' }).click();
  await page.getByRole('option', { name: 'No public holiday' }).click();
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(page.getByRole('status')).toHaveText(`${site} was created.`);

  // A person on that site.
  await page.goto('/workload/resources');
  await page.getByRole('button', { name: 'New resource' }).click();
  dialog = page.getByRole('dialog');
  await dialog.getByLabel('Name').fill(name);
  await dialog.getByRole('combobox', { name: 'Site' }).click();
  await page.getByRole('option', { name: site }).click();
  await dialog.getByRole('button', { name: 'Create' }).click();
  await page.getByRole('link', { name }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(name);
  await expect(page.getByText(`Site ${site}`)).toBeVisible();

  // Part time since January.
  const capacity = page.locator('app-capacity-card');
  await capacity.getByRole('button', { name: 'New period' }).click();
  dialog = page.getByRole('dialog');
  await dialog.getByLabel('From').fill(`1/1/${now.getFullYear()}`);
  await dialog.getByLabel('Wednesday').fill('0');
  await dialog.getByLabel('Friday').fill('3,5');
  await dialog.getByRole('button', { name: 'Add' }).click();
  await expect(capacity.getByRole('status')).toHaveText('The period was added.');

  // A day of leave, then a half day.
  const absences = page.locator('app-absences-card');
  for (const [day, afternoon] of [
    [thursday, false],
    [friday, true],
  ] as const) {
    await page.getByRole('button', { name: 'Declare an absence' }).first().click();
    dialog = page.getByRole('dialog');
    await dialog.getByRole('combobox', { name: 'Kind' }).click();
    await page.getByRole('option', { name: 'Leave' }).click();
    await dialog.getByLabel('First day').fill(typed(day));
    if (afternoon) await dialog.getByRole('switch', { name: /Starts at noon/ }).click();
    await dialog.getByRole('button', { name: 'Declare' }).click();
    await expect(absences.getByRole('status')).toHaveText('The absence was declared.');
    await expect(dialog).toHaveCount(0);
  }

  // The next 4 weeks, from this week's Monday.
  const monday = today - weekday(today) * DAY;
  const weekly = [0, 1, 2, 3].map((week) =>
    hours(Array.from({ length: 7 }, (_, i) => minutesOn(monday + (week * 7 + i) * DAY)).reduce((sum, m) => sum + m, 0)),
  );
  const weeks = page.locator('app-capacity-weeks');
  await expect(weeks.locator('.week')).toHaveCount(4);
  await expect(weeks.locator('.hours')).toHaveText(weekly);

  // Next month in the grid.
  const calendar = page.locator('app-month-calendar');
  const title = calendar.getByRole('heading', { level: 2 });
  await expect(title).not.toBeEmpty();
  const shown = (await title.textContent())?.trim() ?? '';
  await calendar.getByRole('button', { name: 'Next month' }).click();
  await expect(title).not.toHaveText(shown);
  const cell = (time: number) => calendar.locator(`[data-day="${iso(time)}"]`);
  await expect(cell(thursday)).toHaveAttribute('data-look', 'absence');
  await expect(cell(thursday).locator('.tag')).toHaveText(['Leave']);
  await expect(cell(friday)).toHaveAttribute('data-look', 'absence');
  await expect(cell(friday).locator('.tag')).toHaveText(['Leave', '1 h 45 min']);
  await expect(cell(friday)).toHaveAccessibleName(/afternoon, 1 h 45 min$/);
  // The Wednesday after, off; the Monday after, a full day.
  await expect(cell(thursday + 6 * DAY)).toHaveAttribute('data-look', 'off');
  await expect(cell(thursday + 4 * DAY).locator('.tag')).toHaveText('7 h');

  const daysInMonth = new Date(Date.UTC(now.getFullYear(), now.getMonth() + 2, 0)).getUTCDate();
  const total = Array.from({ length: daysInMonth }, (_, i) => minutesOn(nextMonth + i * DAY)).reduce((sum, m) => sum + m, 0);
  await expect(calendar.locator('.total')).toHaveText(`Capacity this month: ${hours(total)}`);
  expect(await accessibilityViolations(page), 'resource page with its capacity').toEqual([]);
  await context.close();
});
