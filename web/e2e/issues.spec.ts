import type { Page } from '@playwright/test';

import { accessibilityViolations, expect, problem, test } from './fixtures';

/**
 * Tickets (L05-US01), with the API mocked: a member creates a ticket from
 * "My tickets" with a rich description and a required field, then lands on
 * its page; axe-core on every screen, in both themes and languages.
 */

const PROJECT = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7c03',
  type: 'project',
  name: 'Identity platform',
  key: 'PLAT',
  description: '',
  objectives: '',
  startDate: null,
  endDate: null,
  health: 'on_track',
  archived: false,
  confidential: false,
  owners: [],
  parents: [],
  version: 1,
};

const TYPE = (id: string, key: string, en: string, color: string, icon: string, parentIds: string[], isSubtask = false) => ({
  id,
  key,
  label: en,
  labelTranslations: { en },
  icon,
  color,
  isSubtask,
  parentIds,
});

const TYPES = [
  TYPE('t-epic', 'epic', 'Epic', 'epic', 'bolt', []),
  TYPE('t-story', 'story', 'Story', 'story', 'bookmark', ['t-epic']),
  TYPE('t-bug', 'bug', 'Bug', 'bug', 'bug_report', ['t-epic']),
  TYPE('t-sub', 'subtask', 'Sub-task', 'task', 'subdirectory_arrow_right', ['t-story', 't-bug'], true),
];

const FORM = {
  project: { id: PROJECT.id, key: 'PLAT', name: PROJECT.name },
  types: TYPES,
  typeId: 't-story',
  fields: [
    {
      fieldId: 'f-app',
      key: 'application',
      label: 'Application',
      kind: 'single_select',
      mode: 'required',
      values: [
        { id: 'v-dir', label: 'Directory', children: [] },
        { id: 'v-mail', label: 'Mail', children: [] },
      ],
    },
    { fieldId: 'f-labels', key: 'labels', label: 'Labels', kind: 'labels', mode: 'visible' },
    { fieldId: 'f-points', key: 'points', label: 'Estimate', kind: 'number', mode: 'visible', decimals: 0 },
  ],
};

const CRM = { ...PROJECT, id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7c09', name: 'Customer relations', key: 'CRM' };

const ISSUE = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7e01',
  key: 'PLAT-142',
  project: FORM.project,
  type: TYPES[1],
  status: { id: 's1', key: 'todo', label: 'To do', category: 'todo' },
  summary: 'Migrate the authentication to OIDC',
  description: null,
  descriptionHtml:
    '<p>Replace the <strong>local</strong> accounts by <a href="https://example.org/oidc" rel="noopener noreferrer nofollow" target="_blank">OIDC</a>.</p><ul><li><p>Entra ID</p></li><li><p>Fallback for contractors</p></li></ul>',
  priority: 'high',
  reporter: { id: 'a1', name: 'Lucas Bernard' },
  assignee: { id: 'a2', name: 'Camille Martin' },
  startDate: '2026-11-09',
  dueDate: '2026-12-04',
  fields: [
    { fieldId: 'f-app', key: 'application', label: 'Application', kind: 'single_select', value: 'v-dir', display: 'Directory' },
    { fieldId: 'f-points', key: 'points', label: 'Estimate', kind: 'number', value: 8, display: null },
  ],
  parentId: null,
  createdAt: '2026-11-06T09:12:00Z',
  updatedAt: '2026-11-06T18:20:00Z',
  version: 1,
};

const MOVE_PLAN = {
  project: { id: CRM.id, key: 'CRM', name: CRM.name },
  sameConfiguration: false,
  tickets: [{ id: ISSUE.id, key: ISSUE.key, summary: ISSUE.summary }],
  parentCleared: null,
  targets: [
    { id: 'c-story', label: 'User story', isSubtask: false, parentIds: [], statusIds: ['c-open'] },
    { id: 'c-incident', label: 'Incident', isSubtask: false, parentIds: [], statusIds: ['c-open'] },
  ],
  statusTargets: [{ id: 'c-open', label: 'Open', category: 'todo' }],
  types: [{ id: 't-story', key: 'story', label: 'Story', isSubtask: false, suggestedId: 'c-story' }],
  statuses: [{ id: 's1', key: 'todo', label: 'To do', category: 'todo', typeIds: ['t-story'], suggestedId: 'c-open' }],
  droppedFields: [{ fieldId: 'f-app', label: 'Application' }],
  problems: [],
};

const SUMMARY = {
  id: ISSUE.id,
  key: ISSUE.key,
  summary: ISSUE.summary,
  project: ISSUE.project,
  type: ISSUE.type,
  status: ISSUE.status,
  priority: ISSUE.priority,
  assignee: ISSUE.assignee,
  updatedAt: ISSUE.updatedAt,
};

/**
 * Animations finished and fonts loaded: axe then measures the final colors
 * and tells icon ligatures (`dark_mode`) from text only once the icon font
 * is applied.
 */
async function animationsDone(page: Page) {
  await page.evaluate(() => document.fonts.ready);
  await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== 'running'));
}

/** The API of tickets; what was posted, kept. */
async function mockTickets(page: Page, mine = [SUMMARY]) {
  const posted: unknown[] = [];
  // No transition unless a test offers some (mockTransitions, registered later, wins).
  await page.route('**/api/v1/issues/*/transitions', (route) => route.fulfill({ json: [] }));
  await page.route(`**/api/v1/nodes/${CRM.id}/my-permissions`, (route) => route.fulfill({ json: ['view'] }));
  // No comment unless a test writes some (mockComments, registered later, wins).
  await page.route('**/api/v1/issues/*/comments', (route) => route.fulfill({ json: [] }));
  // No history unless a test tells some (mockActivity, registered later, wins).
  await page.route('**/api/v1/issues/*/activity**', (route) => route.fulfill({ json: { items: [], nextCursor: null } }));
  // No attachment unless a test adds some (mockAttachments, registered later, wins).
  await page.route('**/api/v1/issues/*/attachments', (route) => route.fulfill({ json: [] }));
  // The member watches the ticket.
  await page.route('**/api/v1/issues/*/watchers', (route) => route.fulfill({ json: { count: 1, watching: true } }));
  await page.route('**/api/v1/me/issues**', (route) => route.fulfill({ json: { items: mine, nextCursor: null } }));
  await page.route('**/api/v1/nodes?**', (route) => route.fulfill({ json: { items: [PROJECT, CRM], nextCursor: null } }));
  await page.route('**/api/v1/projects/PLAT/issue-form**', (route) => route.fulfill({ json: FORM }));
  await page.route('**/api/v1/projects/PLAT/people**', (route) => route.fulfill({ json: [{ id: 'a2', name: 'Camille Martin' }] }));
  await page.route('**/api/v1/projects/PLAT/issues', (route) => {
    const body = route.request().postDataJSON() as { fields: Record<string, unknown> };
    posted.push(body);
    return body.fields['f-app'] ? route.fulfill({ status: 201, json: ISSUE }) : route.fulfill(problem(400, 'validation_failed', [{ field: 'fields.f-app', code: 'required' }]));
  });
  await page.route('**/api/v1/issues/PLAT-142**', (route) => route.fulfill({ json: ISSUE }));
  await page.route(`**/api/v1/nodes/${PROJECT.id}/my-permissions`, (route) => route.fulfill({ json: ['view', 'create_ticket', 'edit', 'comment'] }));
  await page.route(`**/api/v1/issues/${ISSUE.id}/edit-form`, (route) => route.fulfill({ json: { fields: FORM.fields } }));
  await page.route(`**/api/v1/issues/${ISSUE.id}/move-plan`, (route) => route.fulfill({ json: MOVE_PLAN }));
  return posted;
}

test('a member creates a ticket from "My tickets", then lands on its page', async ({ page }) => {
  const posted = await mockTickets(page, []);
  await page.goto('/tickets');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('My tickets');
  await expect(page.getByText('No ticket yet')).toBeVisible();
  await page.getByRole('button', { name: 'New ticket' }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('combobox', { name: 'Project' }).click();
  await page.getByRole('option', { name: 'Identity platform (PLAT)' }).click();
  await expect(dialog.getByRole('combobox', { name: 'Type' })).toContainText('Story');
  await dialog.getByLabel('Summary').fill('Migrate the authentication to OIDC');
  await dialog.getByRole('textbox', { name: 'Description' }).click();
  await page.keyboard.type('Replace the local accounts.');
  // Formatted from the toolbar.
  await dialog.getByRole('button', { name: 'Bulleted list' }).click();
  await dialog.getByRole('button', { name: 'Create' }).click();
  // Required by the type: said next to it, nothing sent.
  await expect(dialog.getByText('This field is required.')).toBeVisible();
  expect(posted).toEqual([]);
  await dialog.getByRole('combobox', { name: 'Application' }).click();
  await page.getByRole('option', { name: 'Mail' }).click();
  await dialog.getByRole('button', { name: 'Create' }).click();

  await expect(page).toHaveURL(/\/issues\/PLAT-142$/);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Migrate the authentication to OIDC');
  expect(posted).toEqual([
    expect.objectContaining({
      typeId: 't-story',
      priority: 'medium',
      description: {
        type: 'doc',
        content: [{ type: 'bulletList', content: [{ type: 'listItem', content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Replace the local accounts.' }] }] }] }],
      },
      fields: { 'f-app': 'v-mail' },
    }),
  ]);
});

test('a link in the description cannot point to a script', async ({ page }) => {
  await mockTickets(page);
  await page.goto('/tickets');
  await page.getByRole('button', { name: 'New ticket' }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('combobox', { name: 'Project' }).click();
  await page.getByRole('option', { name: 'Identity platform (PLAT)' }).click();
  await dialog.getByRole('textbox', { name: 'Description' }).click();
  await page.keyboard.type('docs');
  await page.keyboard.press('ControlOrMeta+a');
  await dialog.getByRole('button', { name: 'Link', exact: true }).click();
  await dialog.getByLabel('Link address').fill('javascript:alert(1)');
  await dialog.getByLabel('Link address').press('Enter');
  await expect(dialog.getByRole('alert')).toContainText('Enter a web address');
  await expect(dialog.getByRole('textbox', { name: 'Description' }).locator('a')).toHaveCount(0);
  await dialog.getByLabel('Link address').fill('https://example.org/docs');
  await dialog.getByLabel('Link address').press('Enter');
  await expect(dialog.getByRole('textbox', { name: 'Description' }).locator('a')).toHaveAttribute('href', 'https://example.org/docs');
});

test('a member edits a ticket, is told someone saved first, reloads and saves again', async ({ page }) => {
  await mockTickets(page);
  const sent: { ifMatch: string | undefined; body: unknown }[] = [];
  await page.route(`**/api/v1/issues/${ISSUE.id}`, (route) => {
    sent.push({ ifMatch: route.request().headers()['if-match'], body: route.request().postDataJSON() });
    return sent.length === 1
      ? route.fulfill(problem(412, 'precondition_failed'))
      : route.fulfill({ json: { ...ISSUE, summary: 'Migrate to OIDC', priority: 'highest', version: 3 } });
  });
  await page.goto('/issues/PLAT-142');
  await page.getByRole('button', { name: 'Edit' }).click();
  const dialog = page.getByRole('dialog', { name: 'Edit PLAT-142' });
  await expect(dialog.getByLabel('Summary')).toHaveValue('Migrate the authentication to OIDC');
  await expect(dialog.getByRole('combobox', { name: 'Application' })).toContainText('Directory');
  await dialog.getByLabel('Summary').fill('Migrate to OIDC');
  await dialog.getByRole('button', { name: 'Save' }).click();
  await expect(dialog.getByRole('alert')).toContainText('Someone changed this ticket since you opened it');
  await dialog.getByRole('button', { name: 'Reload' }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);

  await page.getByRole('button', { name: 'Edit' }).click();
  await dialog.getByLabel('Summary').fill('Migrate to OIDC');
  await dialog.getByRole('combobox', { name: 'Priority' }).click();
  await page.getByRole('option', { name: 'Highest' }).click();
  await dialog.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Migrate to OIDC');
  expect(sent).toEqual([
    { ifMatch: '"1"', body: { summary: 'Migrate to OIDC' } },
    { ifMatch: '"1"', body: { summary: 'Migrate to OIDC', priority: 'highest' } },
  ]);
});

test('a member moves a ticket to another project, its type and status mapped', async ({ page }) => {
  await mockTickets(page);
  let moved: unknown;
  await page.route(`**/api/v1/issues/${ISSUE.id}/move`, (route) => {
    moved = route.request().postDataJSON();
    return route.fulfill({ json: { ...ISSUE, key: 'CRM-7', project: MOVE_PLAN.project, version: 2 } });
  });
  await page.route('**/api/v1/issues/CRM-7**', (route) => route.fulfill({ json: { ...ISSUE, key: 'CRM-7', project: MOVE_PLAN.project } }));
  await page.goto('/issues/PLAT-142');
  await page.getByRole('button', { name: 'Move' }).click();
  const dialog = page.getByRole('dialog', { name: 'Move PLAT-142 to another project' });
  await dialog.getByRole('combobox', { name: 'Target project' }).click();
  await expect(page.getByRole('option')).toHaveText(['Customer relations (CRM)']);
  await page.getByRole('option', { name: 'Customer relations (CRM)' }).click();
  await expect(dialog.getByRole('combobox', { name: 'Story becomes' })).toContainText('User story');
  await expect(dialog.getByRole('combobox', { name: 'Status To do becomes' })).toContainText('Open');
  await expect(dialog.getByText('their values will be lost: Application')).toBeVisible();
  await dialog.getByRole('button', { name: 'Move' }).click();
  await expect(page).toHaveURL(/\/issues\/CRM-7$/);
  expect(moved).toEqual({
    projectKey: 'CRM',
    typeMappings: [{ fromId: 't-story', toId: 'c-story' }],
    statusMappings: [{ fromId: 's1', toId: 'c-open' }],
  });
});

const RELATED = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7e24',
  key: 'CRM-224',
  summary: 'Customer portal forms',
  project: { id: CRM.id, key: 'CRM', name: CRM.name },
  type: TYPES[1],
  status: { id: 's2', key: 'in_progress', label: 'In progress', category: 'in_progress' },
};

/** The ticket with a sub-task and a link, and the link API; what was posted, kept. */
async function mockLinks(page: Page) {
  const posted: unknown[] = [];
  const subtask = { ...RELATED, id: 'sub', key: 'PLAT-143', summary: 'Configure the client', project: FORM.project, type: TYPES[3] };
  const withRelations = {
    ...ISSUE,
    children: [subtask],
    links: [{ id: 'l1', relation: 'depends_on', issue: { ...RELATED, id: 'dep', key: 'PLAT-120', summary: 'Directory scoping', project: FORM.project } }],
  };
  await page.route('**/api/v1/issues/PLAT-142**', (route) => route.fulfill({ json: withRelations }));
  await page.route('**/api/v1/issues?**', (route) => route.fulfill({ json: [RELATED] }));
  await page.route(`**/api/v1/issues/${ISSUE.id}/links`, (route) => {
    posted.push(route.request().postDataJSON());
    return route.fulfill({ status: 201, json: { id: 'l2', relation: 'blocks', issue: RELATED } });
  });
  return posted;
}

test('a member links a ticket of another project, found by its summary', async ({ page }) => {
  await mockTickets(page);
  const posted = await mockLinks(page);
  await page.goto('/issues/PLAT-142');
  const card = page.locator('app-issue-relations');
  await expect(card.locator('.line .key')).toHaveText(['PLAT-143', 'PLAT-120']);
  await expect(card.locator('.line .relation')).toHaveText(['Sub-task', 'Depends on']);
  await page.getByRole('button', { name: 'Link a ticket' }).click();
  const dialog = page.getByRole('dialog', { name: 'Link PLAT-142 to a ticket' });
  await dialog.getByRole('combobox', { name: 'This ticket' }).click();
  await page.getByRole('option', { name: 'Blocks', exact: true }).click();
  await dialog.getByRole('combobox', { name: 'Ticket', exact: true }).fill('portal');
  await page.getByRole('option', { name: /CRM-224 Customer portal forms/ }).click();
  await dialog.getByRole('button', { name: 'Link', exact: true }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(posted).toEqual([{ relation: 'blocks', issue: RELATED.id }]);
});

const TRANSITIONS = [
  { id: 'tr-start', name: 'Start', toStatus: { id: 's2', key: 'in_progress', label: 'In progress', category: 'in_progress' }, fields: [] },
  {
    id: 'tr-review',
    name: 'Send to review',
    toStatus: { id: 's3', key: 'in_review', label: 'In review', category: 'in_progress' },
    fields: [{ fieldId: 'f-points', key: 'points', label: 'Estimate', kind: 'number', mode: 'required', decimals: 0 }],
  },
];

/** The transitions of the ticket; what was run, kept. */
async function mockTransitions(page: Page) {
  const run: unknown[] = [];
  await page.route(`**/api/v1/issues/${ISSUE.id}/transitions`, (route) => {
    if (route.request().method() === 'GET') return route.fulfill({ json: TRANSITIONS });
    run.push(route.request().postDataJSON());
    return route.fulfill({ json: { ...ISSUE, status: TRANSITIONS[1].toStatus, version: 2 } });
  });
  return run;
}

test('a member sends a ticket to review, filling the field the transition takes', async ({ page }) => {
  await mockTickets(page);
  const run = await mockTransitions(page);
  await page.goto('/issues/PLAT-142');
  await expect(page.getByRole('button', { name: 'Start', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Status: To do. Change it' }).click();
  await page.getByRole('menuitem', { name: /Send to review/ }).click();
  const dialog = page.getByRole('dialog', { name: 'Send to review' });
  await expect(dialog.getByText('The ticket goes to “In review”.')).toBeVisible();
  await dialog.getByLabel('Estimate').fill('5');
  await dialog.getByRole('button', { name: 'Send to review' }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(run).toEqual([{ transitionId: 'tr-review', fields: { 'f-points': 5 } }]);
  await expect(page.getByRole('button', { name: 'Status: In review. Change it' })).toBeVisible();
});

const COMMENT = {
  id: 'c1',
  author: { id: 'a2', name: 'Camille Martin' },
  visibility: 'public',
  byTransition: false,
  content: null,
  contentHtml: '<p><span class="mention" data-id="a1">@Lucas Bernard</span> see <a class="issue-ref" href="/issues/PLAT-120">PLAT-120</a></p>',
  deleted: false,
  createdAt: '2026-11-10T09:00:00Z',
  editedAt: null,
  version: 1,
  canEdit: false,
  canDelete: true,
};

/** The comments of the ticket; what was written, kept. */
async function mockComments(page: Page) {
  const written: unknown[] = [];
  await page.route(`**/api/v1/issues/${ISSUE.id}/comments`, (route) => {
    if (route.request().method() === 'GET') return route.fulfill({ json: written.length ? [COMMENT, { ...COMMENT, id: 'c2', canEdit: true, author: { id: 'a1', name: 'Lucas Bernard' }, contentHtml: '<p>Done</p>' }] : [COMMENT] });
    written.push(route.request().postDataJSON());
    return route.fulfill({ status: 201, json: { ...COMMENT, id: 'c2' } });
  });
  await page.route('**/api/v1/projects/PLAT/people**', (route) =>
    route.fulfill({ json: [{ id: 'a2', name: 'Camille Martin' }, { id: 'a3', name: 'Hugo Leroy' }] }),
  );
  return written;
}

/** The history of the ticket: a change through MCP, a status, a description, the creation. */
async function mockActivity(page: Page) {
  const entry = (id: string, rest: object) => ({
    id,
    changeId: `c-${id}`,
    action: 'updated',
    field: null,
    fieldLabel: null,
    relation: null,
    old: null,
    new: null,
    actor: { id: 'a1', name: 'Lucas Bernard' },
    origin: 'interface',
    createdAt: '2026-11-06T08:12:00Z',
    ...rest,
  });
  const text = (value: string) => ({ kind: 'text', text: value, html: null });
  await page.route(`**/api/v1/issues/${ISSUE.id}/activity**`, (route) =>
    route.fulfill({
      json: {
        items: [
          entry('e4', { field: 'priority', old: { kind: 'priority', text: 'medium', html: null }, new: { kind: 'priority', text: 'high', html: null }, actor: { id: 'a2', name: 'Camille Martin' }, origin: 'mcp' }),
          entry('e3', { field: 'status', old: text('To do'), new: text('In progress') }),
          entry('e2', { field: 'description', new: { kind: 'richText', text: null, html: '<p>Use <strong>OIDC</strong>.</p>' } }),
          entry('e1', { action: 'created', createdAt: '2026-11-02T08:00:00Z' }),
        ],
        nextCursor: null,
      },
    }),
  );
}

test('the Activity tab tells who changed what, through MCP or not, and a description before and after', async ({ page }) => {
  await mockTickets(page);
  await mockActivity(page);
  await page.goto('/issues/PLAT-142');
  await expect(page.getByRole('tab', { name: 'Activity' })).toHaveAttribute('aria-selected', 'true');
  const entries = page.locator('app-issue-activity .entry .sentence');
  await expect(entries).toHaveText([
    /^Camille Martin changed the priority: Medium → High\s+smart_toy\s+via MCP\s*$/,
    'Lucas Bernard changed the status: To do → In progress',
    'Lucas Bernard changed the description',
    'Lucas Bernard created the ticket',
  ]);
  const toggle = page.getByRole('button', { name: 'Show before and after' });
  await toggle.click();
  await expect(page.getByRole('button', { name: 'Hide before and after' })).toHaveAttribute('aria-expanded', 'true');
  await expect(page.locator('app-issue-activity .change .side').nth(1)).toContainText('Use OIDC.');
});

/** The attachments of the ticket: a PDF and an image already there; a file sent is scanned on the next read. */
async function mockAttachments(page: Page) {
  const file = (id: string, rest: object) => ({
    id,
    name: `${id}.pdf`,
    size: 3_145_728,
    mediaType: 'application/pdf',
    status: 'clean',
    signature: null,
    uploadedBy: { id: 'a1', name: 'Lucas Bernard' },
    createdAt: '2026-11-10T09:00:00Z',
    hasThumbnail: false,
    canDelete: true,
    ...rest,
  });
  const files = [file('plan', {}), file('infected', { name: 'invoice.exe', status: 'infected', signature: 'Eicar-Test-Signature', canDelete: false })];
  const sent: { body: unknown; uploaded: string[] } = { body: null, uploaded: [] };
  await page.route(`**/api/v1/issues/${ISSUE.id}/attachments`, (route) => {
    if (route.request().method() === 'POST') {
      sent.body = route.request().postDataJSON();
      return route.fulfill({ status: 201, json: { attachment: file('notes', { name: 'notes.txt', status: 'pending' }), uploadUrl: '/mlbpp-attachments/t/uploads/notes?X-Amz-Signature=x' } });
    }
    return route.fulfill({ json: sent.uploaded.length ? [...files, file('notes', { name: 'notes.txt', mediaType: 'text/plain', size: 12, status: 'scanning' })] : files });
  });
  await page.route('**/mlbpp-attachments/**', (route) => {
    sent.uploaded.push(route.request().method());
    return route.fulfill({ status: 200, body: '' });
  });
  await page.route(`**/api/v1/issues/${ISSUE.id}/attachments/notes/uploaded`, (route) =>
    route.fulfill({ json: file('notes', { name: 'notes.txt', status: 'scanning' }) }),
  );
  return sent;
}

test('a member attaches a file, sent straight to the storage, then scanned', async ({ page }) => {
  await mockTickets(page);
  const sent = await mockAttachments(page);
  await page.goto('/issues/PLAT-142');
  const card = page.locator('app-issue-attachments');
  await expect(card.getByRole('heading', { name: 'Attachments (2)' })).toBeVisible();
  await expect(card.getByRole('link', { name: 'plan.pdf' })).toHaveAttribute('href', `/api/v1/issues/${ISSUE.id}/attachments/plan/content`);
  await expect(card.locator('[data-status="infected"] .scan')).toHaveText('Infected');
  await card.locator('input[type="file"]').setInputFiles({ name: 'notes.txt', mimeType: 'text/plain', buffer: Buffer.from('Recette OK !') });
  await expect(card.locator('[data-status="scanning"] .scan')).toHaveText('Scanning');
  expect(sent.body).toEqual({ name: 'notes.txt', size: 12, mediaType: 'text/plain' });
  expect(sent.uploaded).toEqual(['PUT']);
});

test('a member comments with a mention picked with the keyboard', async ({ page }) => {
  await mockTickets(page);
  const written = await mockComments(page);
  await page.goto('/issues/PLAT-142');
  // The comments are in their tab, next to the activity (L05-US06).
  await page.getByRole('tab', { name: 'Comments (1)' }).click();
  const section = page.locator('app-issue-comments');
  await expect(section.locator('.mention')).toHaveText('@Lucas Bernard');
  const editor = section.getByRole('textbox', { name: 'Add a comment' });
  await editor.click();
  await page.keyboard.type('Thanks @');
  const list = section.getByRole('listbox');
  await expect(list.getByRole('option')).toHaveCount(2);
  await page.keyboard.press('ArrowDown');
  await expect(list.getByRole('option').nth(1)).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('Enter');
  await expect(list).toHaveCount(0);
  await expect(editor.locator('.mention')).toHaveText('@Hugo Leroy');
  await section.getByRole('button', { name: 'Comment', exact: true }).click();
  await expect(section.locator('.comment')).toHaveCount(2);
  expect(written).toEqual([
    {
      content: {
        type: 'doc',
        content: [
          {
            type: 'paragraph',
            content: [
              { type: 'text', text: 'Thanks ' },
              expect.objectContaining({ type: 'mention', attrs: expect.objectContaining({ id: 'a3', label: 'Hugo Leroy' }) }),
              // The space the editor adds after a mention.
              { type: 'text', text: ' ' },
            ],
          },
        ],
      },
      visibility: 'internal',
    },
  ]);
});

for (const theme of ['light', 'dark'] as const) {
  for (const language of ['fr', 'en'] as const) {
    test.describe(`accessibility of tickets, ${theme} mode, ${language}`, () => {
      test.use({ theme, language });

      test('"My tickets", the creation dialog and a ticket have no axe-core violation', async ({ page }) => {
        await mockTickets(page);
        await page.goto('/tickets');
        await expect(page.getByRole('link', { name: 'PLAT-142' })).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);

        await page.locator('button.create').click();
        const dialog = page.getByRole('dialog');
        await dialog.locator('mat-select[formcontrolname="project"]').click();
        await page.getByRole('option', { name: 'Identity platform (PLAT)' }).click();
        await expect(dialog.locator('app-field-input')).toHaveCount(3);
        await dialog.locator('button.confirm').click();
        await expect(dialog.locator('mat-error').first()).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
        await page.keyboard.press('Escape');

        await page.goto('/issues/PLAT-142');
        await expect(page.getByRole('heading', { level: 1 })).toHaveText('Migrate the authentication to OIDC');
        await expect(page.locator('ui-rich-text a')).toHaveText('OIDC');
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
      });

      test('the Activity tab, a description before and after open, has no axe-core violation', async ({ page }) => {
        await mockTickets(page);
        await mockActivity(page);
        await page.goto('/issues/PLAT-142');
        await expect(page.locator('app-issue-activity .entry')).toHaveCount(4);
        await page.locator('app-issue-activity button.toggle').click();
        await expect(page.locator('app-issue-activity .change')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
      });

      test('the attachments, a refused file among them, have no axe-core violation', async ({ page }) => {
        await mockTickets(page);
        await mockAttachments(page);
        await page.route(`**/api/v1/issues/${ISSUE.id}/attachments`, (route) =>
          route.request().method() === 'POST'
            ? route.fulfill({ status: 413, json: { type: 'about:blank', title: 'Too large', status: 413, code: 'attachment.too_large' } })
            : route.fallback(),
        );
        await page.goto('/issues/PLAT-142');
        const card = page.locator('app-issue-attachments');
        await expect(card.locator('.file')).toHaveCount(2);
        await card.locator('input[type="file"]').setInputFiles({ name: 'video.mp4', mimeType: 'video/mp4', buffer: Buffer.from('x') });
        await expect(card.locator('.upload [role="alert"]')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
      });

      test('the comments and the mention suggestions have no axe-core violation', async ({ page }) => {
        await mockTickets(page);
        await mockComments(page);
        await page.goto('/issues/PLAT-142');
        await page.getByRole('tab', { name: /^(Comments|Commentaires) \(1\)$/ }).click();
        await expect(page.locator('app-issue-comments .comment')).toHaveCount(1);
        await page.locator('app-issue-comments .new [role="textbox"]').click();
        await page.keyboard.type('@');
        await expect(page.getByRole('option')).toHaveCount(2);
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
      });

      test('the status menu and the transition dialog have no axe-core violation', async ({ page }) => {
        await mockTickets(page);
        await mockTransitions(page);
        await page.goto('/issues/PLAT-142');
        await page.locator('button.status-menu').click();
        await expect(page.getByRole('menuitem')).toHaveCount(2);
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
        await page.getByRole('menuitem').nth(1).click();
        await expect(page.getByRole('dialog').locator('app-field-input')).toHaveCount(1);
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
      });

      test('the sub-tasks and links card and the link dialog have no axe-core violation', async ({ page }) => {
        await mockTickets(page);
        await mockLinks(page);
        await page.goto('/issues/PLAT-142');
        await expect(page.locator('app-issue-relations .line')).toHaveCount(2);
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
        await page.locator('button.add-link').click();
        const dialog = page.getByRole('dialog');
        await dialog.locator('app-issue-picker input').fill('portal');
        await expect(page.getByRole('option')).toHaveCount(1);
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
      });

      test('the edit and move dialogs have no axe-core violation', async ({ page }) => {
        await mockTickets(page);
        await page.goto('/issues/PLAT-142');
        await page.locator('button.edit').click();
        await expect(page.getByRole('dialog').locator('app-field-input')).toHaveCount(3);
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
        await page.keyboard.press('Escape');
        await expect(page.getByRole('dialog')).toHaveCount(0);

        await page.locator('button.move').click();
        const dialog = page.getByRole('dialog');
        await dialog.locator('mat-select').first().click();
        await page.getByRole('option').first().click();
        await expect(dialog.locator('.dropped')).toBeVisible();
        await animationsDone(page);
        expect(await accessibilityViolations(page)).toEqual([]);
      });
    });
  }
}
