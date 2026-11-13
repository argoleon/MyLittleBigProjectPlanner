/**
 * The hierarchy on the production stack, nothing mocked.
 *
 * - Several parents and the cycle check (L03-US02): the tenant admin builds
 *   a portfolio, two programs, a project and its sub-project, attaches the
 *   project to both programs with weightings, then sees that its
 *   sub-project cannot become its parent.
 * - Roles granted on nodes (L03-US03): a member without any role sees no
 *   node; the admin grants them a role on a portfolio, and they see it with
 *   the project below, until the role is revoked.
 * - Confidential projects (L03-US04): made confidential, the project closes
 *   to the member, who finds it veiled under the portfolio, until they get
 *   a role on the project itself.
 *
 * The tests share the member: they run one after the other.
 *
 * Runs with `just compose-e2e` only: deploy/tests/e2e.sh creates the admin
 * and the member, and passes E2E_ADMIN_EMAIL, E2E_ADMIN_SECRET,
 * E2E_VIEWER_EMAIL and E2E_VIEWER_SECRET.
 */
import { type Browser, type Page, expect, test } from '@playwright/test';

import { accessibilityViolations } from './fixtures';

const adminEmail = process.env['E2E_ADMIN_EMAIL'];
const adminSecret = process.env['E2E_ADMIN_SECRET'];
const viewerEmail = process.env['E2E_VIEWER_EMAIL'];
const viewerSecret = process.env['E2E_VIEWER_SECRET'];

test.skip(!adminEmail || !adminSecret || !viewerEmail || !viewerSecret, 'needs the compose stack (just compose-e2e)');
test.describe.configure({ mode: 'serial' });

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

/** Creates a node from the open dialog, then waits for its settings. */
async function createIn(page: Page, type: string, name: string, key?: string) {
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('combobox', { name: 'Type' }).click();
  await page.getByRole('option', { name: type, exact: true }).click();
  await dialog.getByLabel('Name').fill(name);
  if (key) await dialog.getByLabel('Key').fill(key);
  await dialog.getByRole('button', { name: 'Create' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(`${name} · Settings`);
}

async function addChild(page: Page, type: string, name: string, key?: string) {
  await page.getByRole('button', { name: 'Add a child' }).click();
  await createIn(page, type, name, key);
}

/** On the settings of `node`, grants "Reader" to the member, found by their email. */
async function grantReader(admin: Page, node: string, email: string) {
  await admin.getByRole('tab', { name: 'Attachments and permissions' }).click();
  const access = admin.getByRole('region', { name: 'Effective access' });
  await access.getByRole('button', { name: 'Add a role' }).click();
  const dialog = admin.getByRole('dialog', { name: `Grant a role on ${node}` });
  await dialog.getByLabel('Member or group').fill(email);
  await admin.getByRole('option', { name: email }).click();
  await dialog.getByRole('combobox', { name: 'Role' }).click();
  await admin.getByRole('option', { name: 'Reader', exact: true }).click();
  await dialog.getByRole('button', { name: 'Grant' }).click();
  await expect(access.getByRole('status')).toHaveText('Role Reader granted to E2E Viewer.');
}

test('a project under two weighted programs, never under its own sub-project', async ({ browser }) => {
  test.setTimeout(120_000);
  // Unique per run: the stack keeps its data.
  const run = Date.now().toString(36).toUpperCase().slice(-6);
  const [digital, modernization, compliance] = [`Digital ${run}`, `Modernization ${run}`, `Compliance ${run}`];
  const [platform, permissions] = [`Platform ${run}`, `Permissions ${run}`];

  const page = await signIn(browser, adminEmail!, adminSecret!);

  // A portfolio, a program, a project and its sub-project, each under the last.
  await page.goto('/portfolios');
  await page.getByRole('button', { name: 'New node' }).click();
  await createIn(page, 'Portfolio', digital);
  await addChild(page, 'Program', modernization);
  await addChild(page, 'Project', platform, `P${run}`);
  const platformUrl = page.url();
  await addChild(page, 'Project', permissions, `S${run}`);
  const breadcrumb = page.getByRole('navigation', { name: 'Breadcrumb' });
  // Back up to the portfolio, for the second program.
  await breadcrumb.getByRole('link', { name: platform }).click();
  await breadcrumb.getByRole('link', { name: modernization }).click();
  await breadcrumb.getByRole('link', { name: digital }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText(`${digital} · Settings`);
  await addChild(page, 'Program', compliance);

  // The project: attached under the second program too.
  await page.goto(platformUrl);
  await page.getByRole('tab', { name: 'Attachments' }).click();
  const parents = page.getByRole('region', { name: 'Parents' });
  await expect(parents.getByRole('listitem')).toHaveCount(1);
  await page.getByRole('button', { name: 'Add a parent' }).click();
  const search = page.getByRole('region', { name: 'Add a parent' });
  await search.getByLabel('Search a node').fill(compliance);
  await search.getByRole('button', { name: new RegExp(`^${compliance} Program`) }).click();
  await expect(page.getByRole('status')).toHaveText(`Attached under ${compliance}.`);
  await expect(parents.getByRole('listitem')).toHaveCount(2);

  // Weightings: 40 % to Compliance, Modernization keeps 100 %.
  const weight = parents.getByRole('textbox', { name: `Weighting under ${compliance}` });
  await weight.fill('40');
  await weight.press('Enter');
  await expect(page.getByRole('status')).toHaveText(`Weighting under ${compliance} saved.`);
  await page.reload();
  await expect(parents.getByRole('textbox', { name: `Weighting under ${compliance}` })).toHaveValue('40');
  await expect(parents.getByRole('textbox', { name: `Weighting under ${modernization}` })).toHaveValue('100');
  await expect(page.getByRole('navigation', { name: 'Breadcrumb' })).toContainText(compliance);

  // Its sub-project cannot become its parent.
  await page.getByRole('button', { name: 'Add a parent' }).click();
  await search.getByLabel('Search a node').fill(permissions);
  const refused = search.getByRole('listitem').filter({ hasText: permissions });
  await expect(refused).toContainText('Impossible: it is a descendant of this node, the link would create a cycle');
  await expect(refused.getByRole('button')).toHaveCount(0);
  expect(await accessibilityViolations(page), 'attachments tab').toEqual([]);
  await expect(page.getByRole('region', { name: 'Children' })).toContainText(permissions);
  await page.context().close();
});

test('a role granted on a portfolio opens the project below, until it is revoked', async ({ browser }) => {
  test.setTimeout(120_000);
  const run = Date.now().toString(36).toUpperCase().slice(-6);
  const [portfolio, project] = [`Access ${run}`, `Vault ${run}`];

  // The admin builds a portfolio and a project under it.
  const admin = await signIn(browser, adminEmail!, adminSecret!);
  await admin.goto('/portfolios');
  await admin.getByRole('button', { name: 'New node' }).click();
  await createIn(admin, 'Portfolio', portfolio);
  const portfolioUrl = admin.url();
  await addChild(admin, 'Project', project, `V${run}`);
  const projectUrl = admin.url();

  // Without any role, the member sees no node, not even by its address.
  const viewer = await signIn(browser, viewerEmail!, viewerSecret!);
  await viewer.goto('/portfolios');
  await expect(viewer.getByText('No portfolio, program or project yet')).toBeVisible();
  await viewer.goto(projectUrl);
  await expect(viewer.getByRole('alert')).toContainText('This item does not exist or is no longer available.');

  // The admin grants them "Reader" on the portfolio.
  await admin.goto(portfolioUrl);
  await admin.getByRole('tab', { name: 'Attachments and permissions' }).click();
  const access = admin.getByRole('region', { name: 'Effective access' });
  await expect(access).toContainText('Nobody has a role here yet.');
  await grantReader(admin, portfolio, viewerEmail!);
  await expect(access.getByRole('listitem')).toContainText(['E2E Viewer']);
  await expect(access.getByRole('listitem')).toContainText(['Direct role on this portfolio']);
  expect(await accessibilityViolations(admin), 'effective access').toEqual([]);
  // On the project below, the role shows as inherited from the portfolio.
  await admin.goto(projectUrl);
  await admin.getByRole('tab', { name: 'Attachments and permissions' }).click();
  await expect(admin.getByRole('region', { name: 'Effective access' }).getByRole('listitem')).toContainText([
    `Inherited from the portfolio ${portfolio}`,
  ]);

  // The member now sees the portfolio and the project below it, as a reader.
  await viewer.goto('/portfolios');
  await expect(viewer.getByRole('link', { name: portfolio })).toBeVisible();
  await viewer.getByRole('link', { name: project }).click();
  await expect(viewer.getByRole('heading', { level: 1 })).toHaveText(`${project} · Settings`);
  await viewer.getByRole('tab', { name: 'Attachments and permissions' }).click();
  await expect(viewer.getByRole('region', { name: 'Parents' })).toBeVisible();
  await expect(viewer.getByRole('region', { name: 'Effective access' })).toHaveCount(0);

  // The role revoked on the portfolio: gone at their next visit.
  await admin.goto(portfolioUrl);
  await admin.getByRole('tab', { name: 'Attachments and permissions' }).click();
  await access.getByRole('button', { name: 'Revoke the role Reader of E2E Viewer' }).click();
  await expect(access.getByRole('status')).toHaveText('Role Reader revoked from E2E Viewer.');
  await viewer.goto('/portfolios');
  await expect(viewer.getByText('No portfolio, program or project yet')).toBeVisible();
  await viewer.goto(projectUrl);
  await expect(viewer.getByRole('alert')).toContainText('This item does not exist or is no longer available.');

  await admin.context().close();
  await viewer.context().close();
});

test('a confidential project closes to those above it, who find it veiled', async ({ browser }) => {
  test.setTimeout(120_000);
  const run = Date.now().toString(36).toUpperCase().slice(-6);
  const [portfolio, project] = [`Vaults ${run}`, `Secret ${run}`];

  // A portfolio the member reads, and a project under it.
  const admin = await signIn(browser, adminEmail!, adminSecret!);
  await admin.goto('/portfolios');
  await admin.getByRole('button', { name: 'New node' }).click();
  await createIn(admin, 'Portfolio', portfolio);
  await grantReader(admin, portfolio, viewerEmail!);
  await addChild(admin, 'Project', project, `X${run}`);
  const projectUrl = admin.url();
  const viewer = await signIn(browser, viewerEmail!, viewerSecret!);
  await viewer.goto('/portfolios');
  await expect(viewer.getByRole('link', { name: project })).toBeVisible();

  // The admin makes the project confidential: the inherited role stops there.
  await admin.getByRole('tab', { name: 'Attachments and permissions' }).click();
  const access = admin.getByRole('region', { name: 'Effective access' });
  await expect(access.getByRole('listitem')).toContainText([`Inherited from the portfolio ${portfolio}`]);
  const confidentiality = admin.getByRole('region', { name: 'Confidentiality' });
  await confidentiality.getByRole('switch', { name: 'Confidential project' }).click();
  await expect(confidentiality.getByRole('status')).toHaveText('The project is now confidential.');
  await expect(confidentiality.getByRole('switch')).toBeChecked();
  await expect(access).toContainText('Nobody has a role here yet.');
  expect(await accessibilityViolations(admin), 'confidentiality').toEqual([]);

  // The member no longer reads it: it shows veiled under the portfolio.
  await viewer.goto('/portfolios');
  const row = viewer.getByRole('row').filter({ hasText: portfolio }).filter({ hasText: 'Confidential project' });
  await expect(row).toContainText('You do not have access.');
  await expect(row.getByRole('img', { name: 'Locked' })).toBeVisible();
  await expect(viewer.getByText(project)).toHaveCount(0);
  expect(await accessibilityViolations(viewer), 'veiled row').toEqual([]);
  await viewer.goto(projectUrl);
  await expect(viewer.getByRole('alert')).toContainText('This item does not exist or is no longer available.');
  await viewer.goto('/portfolios');
  await viewer.getByRole('link', { name: portfolio }).click();
  await viewer.getByRole('tab', { name: 'Attachments and permissions' }).click();
  const children = viewer.getByRole('region', { name: 'Children' });
  await expect(children).toContainText('Confidential project');
  await expect(children.getByRole('link')).toHaveCount(0);

  // A role on the project itself opens it to them again.
  await grantReader(admin, project, viewerEmail!);
  await expect(access.getByRole('listitem')).toContainText(['Direct role on this project']);
  await viewer.goto('/portfolios');
  await viewer.getByRole('link', { name: project }).click();
  await expect(viewer.getByRole('heading', { level: 1 })).toHaveText(`${project} · Settings`);

  await admin.context().close();
  await viewer.context().close();
});
