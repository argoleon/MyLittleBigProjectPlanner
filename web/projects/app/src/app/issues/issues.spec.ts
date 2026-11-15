/**
 * Tests of tickets in the application (L05-US01): the values of custom
 * fields as the API takes them, the types offered, then with the API
 * mocked at the HTTP level and the real translations: the creation dialog
 * from a project, as a sub-task and from "My tickets" (fields of the type's
 * creation screen, errors next to their fields, the ticket's page opened),
 * the ticket's page, "My tickets", and accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { Component, inject } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { FormControl } from '@angular/forms';
import { MatDialog } from '@angular/material/dialog';
import { Router, provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { AuthService, Issue, IssueForm, IssueFormField, IssueSummary, IssueTypeRef, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { errorKeyOf } from './field-input';
import { apiValue, chipCategory, chipKind, controlOf, emptyOf, isEmpty, isIssueKey, typesFor } from './issue-fields';
import { IssueDialogData, createIssueFlow } from './issue-flow';
import { IssuePage } from './issue-page';
import { MyTicketsPage } from './my-tickets-page';

const SESSION = (tenantRole: string) => ({
  account: { id: 'a1', email: 'lucas@acme.example', displayName: 'Lucas Bernard', locale: null, tenantRole },
  stage: 'complete',
  csrfToken: 'csrf',
  expiresAt: '2026-11-20T09:00:00Z',
});

const type = (id: string, label: string, rest: Partial<IssueTypeRef> = {}): IssueTypeRef => ({
  id,
  key: label.toLowerCase().replace('-', ''),
  label,
  labelTranslations: { en: label },
  icon: 'bookmark',
  color: 'story',
  isSubtask: false,
  parentIds: [],
  ...rest,
});

const TYPES: IssueTypeRef[] = [
  type('t-epic', 'Epic', { color: 'epic' }),
  type('t-story', 'Story', { parentIds: ['t-epic'] }),
  type('t-bug', 'Bug', { color: 'bug' }),
  type('t-sub', 'Sub-task', { color: 'task', isSubtask: true, parentIds: ['t-story', 't-bug'] }),
];

const field = (fieldId: string, label: string, rest: Partial<IssueFormField> = {}): IssueFormField => ({
  fieldId,
  key: label.toLowerCase(),
  label,
  kind: 'short_text',
  mode: 'visible',
  ...rest,
});

const APPLICATION = field('f-app', 'Application', {
  kind: 'single_select',
  mode: 'required',
  values: [
    { id: 'v-dir', label: 'Directory', children: [] },
    { id: 'v-mail', label: 'Mail', children: [] },
  ],
});

const FORM: IssueForm = {
  project: { id: 'p1', key: 'PLAT', name: 'Identity platform' },
  types: TYPES,
  typeId: 't-epic',
  fields: [APPLICATION, field('f-env', 'Environment', { maxLength: 40 })],
};

const ISSUE: Issue = {
  id: 'i142',
  key: 'PLAT-142',
  project: { id: 'p1', key: 'PLAT', name: 'Identity platform' },
  type: TYPES[1],
  status: { id: 's2', key: 'in_progress', label: 'In progress', category: 'in_progress' },
  summary: 'Migrate the authentication to OIDC',
  description: { type: 'doc', content: [] },
  descriptionHtml: '<p>Replace the <strong>local</strong> accounts.</p>',
  priority: 'high',
  reporter: { id: 'a1', name: 'Lucas Bernard' },
  assignee: { id: 'a2', name: 'Camille Martin' },
  startDate: '2026-11-09',
  dueDate: '2026-12-04',
  fields: [
    { fieldId: 'f-app', key: 'application', label: 'Application', kind: 'single_select', value: 'v-dir', display: 'Directory', references: [] },
    { fieldId: 'f-points', key: 'points', label: 'Estimate', kind: 'number', value: 8, display: null, references: [] },
    { fieldId: 'f-labels', key: 'labels', label: 'Labels', kind: 'labels', value: ['security', 'directory'], display: null, references: [] },
    { fieldId: 'f-ok', key: 'approved', label: 'Approved', kind: 'checkbox', value: true, display: null, references: [] },
  ],
  parent: { id: 'i100', key: 'PLAT-100', summary: 'Unified authentication' },
  parentId: 'i100',
  createdAt: '2026-11-06T09:12:00Z',
  updatedAt: '2026-11-06T18:20:00Z',
  version: 3,
};

const summary = (key: string, title: string): IssueSummary => ({
  id: key,
  key,
  summary: title,
  project: ISSUE.project,
  type: TYPES[2],
  status: { id: 's1', key: 'todo', label: 'To do', category: 'todo' },
  priority: 'medium',
  assignee: null,
  updatedAt: '2026-11-10T10:00:00Z',
});

describe('custom field values', () => {
  it('start empty and travel as the API takes them', () => {
    expect(emptyOf(APPLICATION)).toBeNull();
    expect(emptyOf(field('f', 'F', { kind: 'checkbox' }))).toBe(false);
    expect(emptyOf(field('f', 'F', { kind: 'labels' }))).toEqual([]);
    expect(emptyOf(field('f', 'F', { kind: 'user' }))).toEqual([]);
    expect([null, '  ', [], 0, false].map((v) => isEmpty(v))).toEqual([true, true, true, false, false]);

    const value = (kind: IssueFormField['kind'], raw: unknown, rest: Partial<IssueFormField> = {}) =>
      apiValue(field('f', 'F', { kind, ...rest }), raw as never);
    expect(value('short_text', '  OIDC  ')).toBe('OIDC');
    expect(value('short_text', '   ')).toBeUndefined();
    expect(value('number', 8.5)).toBe(8.5);
    expect(value('checkbox', false)).toBe(false);
    expect(value('date', new Date(2026, 10, 9))).toBe('2026-11-09');
    expect(value('date_time', '2026-11-09T10:30')).toBe(new Date('2026-11-09T10:30').toISOString());
    expect(value('single_select', 'v-mail')).toBe('v-mail');
    expect(value('multi_select', ['a', 'b'])).toEqual(['a', 'b']);
    expect(value('cascading_select', ['erp', 'hr'])).toEqual(['erp', 'hr']);
    expect(value('labels', [])).toBeUndefined();
    const people = [
      { id: 'a2', name: 'Camille' },
      { id: 'a3', name: 'Hugo' },
    ];
    expect(value('user', people, { multiple: true })).toEqual(['a2', 'a3']);
    expect(value('user', people.slice(0, 1))).toBe('a2');
    expect(value('group', [{ id: 'g1', name: 'Platform team' }])).toBe('g1');
  });

  it('are checked as the browser can: required, length, range, ticket keys', () => {
    const required = controlOf(APPLICATION);
    expect(required.errors).toEqual({ required: true });
    required.setValue('v-dir');
    expect(required.valid).toBe(true);
    const checkbox = controlOf(field('f', 'F', { kind: 'checkbox', mode: 'required' }));
    expect(checkbox.valid).toBe(true);
    const text = controlOf(field('f', 'F', { maxLength: 3 }));
    text.setValue('four');
    expect(text.errors).toEqual({ maxlength: { requiredLength: 3, actualLength: 4 } });
    const number = controlOf(field('f', 'F', { kind: 'number', min: 0, max: 10 }));
    number.setValue(11);
    expect(number.errors?.['max']).toBeTruthy();
    const link = controlOf(field('f', 'F', { kind: 'issue_link' }));
    link.setValue('plat 12');
    expect(link.errors).toEqual({ format: true });
    link.setValue('PLAT-12');
    expect(link.valid).toBe(true);
    expect(['PLAT-12', 'AB-1', 'plat-12', 'PLAT-0', 'P-1', 'PLAT12'].map(isIssueKey)).toEqual([true, true, false, false, false, false]);
  });

  it('name their first error by its translation key, once touched', () => {
    const control = new FormControl<string | null>(null, { validators: () => ({ required: true }) });
    expect(errorKeyOf(control)).toBeNull();
    control.markAsTouched();
    expect(errorKeyOf(control)).toBe('errors.field.required');
    control.setErrors({ api: 'errors.field.unknown_member' });
    expect(errorKeyOf(control)).toBe('errors.field.unknown_member');
  });
});

describe('the types offered at creation', () => {
  it('are those that are no sub-task, or those a parent may hold', () => {
    expect(typesFor(TYPES, null).map((t) => t.label)).toEqual(['Epic', 'Story', 'Bug']);
    expect(typesFor(TYPES, 't-story').map((t) => t.label)).toEqual(['Sub-task']);
    expect(typesFor(TYPES, 't-epic').map((t) => t.label)).toEqual(['Story']);
    expect(typesFor(TYPES, 't-sub')).toEqual([]);
  });

  it('show their status and type with the shared chips', () => {
    expect(['todo', 'in_progress', 'done'].map(chipCategory)).toEqual(['todo', 'progress', 'done']);
    expect(['story', 'bug', 'epic', 'task', 'purple'].map(chipKind)).toEqual(['story', 'bug', 'epic', 'task', 'task']);
  });
});

/** The page the dialog leaves to, with the router's state. */
@Component({ template: '' })
class Opener {
  readonly dialog = inject(MatDialog);
  readonly router = inject(Router);
}

describe('tickets in the application', () => {
  async function setUp(tenantRole = 'member') {
    localStorage.clear();
    TestBed.configureTestingModule({
      providers: [
        provideRouter(
          [
            { path: 'start', component: Opener },
            { path: 'tickets', component: MyTicketsPage },
            { path: 'issues/:key', component: IssuePage },
          ],
          withComponentInputBinding(),
        ),
        provideApi(),
        provideHttpClientTesting(),
        provideI18nTesting(),
        provideMaterialDefaults(),
      ],
    });
    const http = TestBed.inject(HttpTestingController);
    const settle = async () => {
      for (let i = 0; i < 3; i++) {
        await new Promise((resolve) => setTimeout(resolve));
        TestBed.tick();
      }
    };
    const reply = async (match: (url: string, method: string) => boolean, body: object | null, status = 200): Promise<TestRequest> => {
      let request: TestRequest | undefined;
      for (let i = 0; i < 50 && !request; i++) {
        [request] = http.match((r) => match(r.urlWithParams, r.method));
        if (!request) await new Promise((resolve) => setTimeout(resolve, 20));
      }
      if (!request) throw new Error('no matching request');
      request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error' });
      await settle();
      return request;
    };
    const loaded = TestBed.inject(AuthService).load();
    await reply((url) => url === '/api/v1/auth/session', SESSION(tenantRole));
    await loaded;
    const harness = await RouterTestingHarness.create();
    const root = () => document.body;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const texts = (selector: string, within: Element = root()) => Array.from(within.querySelectorAll(selector)).map((e) => text(e));
    const click = async (element: Element | null | undefined) => {
      (element as HTMLElement).click();
      await settle();
    };
    const dialog = () => root().querySelector('mat-dialog-container')!;
    const type = async (input: Element | null, value: string) => {
      (input as HTMLInputElement).value = value;
      input!.dispatchEvent(new Event('input'));
      input!.dispatchEvent(new Event('blur'));
      await settle();
    };
    const options = async (select: Element | null) => {
      await click(select?.querySelector('.mat-mdc-select-trigger'));
      // The panel just opened (one closing may linger).
      const panels = root().querySelectorAll('.mat-mdc-select-panel');
      const labels = texts('mat-option', panels[panels.length - 1]);
      await click(root().querySelectorAll('.cdk-overlay-backdrop')[root().querySelectorAll('.cdk-overlay-backdrop').length - 1]);
      return labels;
    };
    const choose = async (select: Element | null, label: string) => {
      await click(select?.querySelector('.mat-mdc-select-trigger'));
      // The last match: a panel closing may linger before the one just opened.
      await click(Array.from(root().querySelectorAll('mat-option')).reverse().find((o) => text(o) === label));
    };
    /** Opens the creation dialog from a page, as the project page or "My tickets" do. */
    const open = async (data: IssueDialogData) => {
      const opener = await harness.navigateByUrl('/start', Opener);
      const created = createIssueFlow(opener.dialog, opener.router, data);
      await settle();
      return { created, router: opener.router };
    };
    return { http, harness, reply, settle, root, text, texts, click, dialog, type, options, choose, open };
  }

  const isForm = (url: string) => url.startsWith('/api/v1/projects/PLAT/issue-form');

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it('creates a ticket from a project with the fields of its type, errors next to them, then opens it', async () => {
    const { reply, dialog, text, texts, click, type, options, choose, open, settle } = await setUp();
    const { created, router } = await open({ projectKey: 'PLAT' });
    // The API's choice of type, offered: one form read.
    await reply(isForm, { ...FORM, typeId: 't-story' });
    expect(text(dialog().querySelector('h2'))).toBe('New ticket');
    expect(dialog().querySelector('mat-select[formcontrolname="project"]')).toBeNull();
    expect(await options(dialog().querySelector('mat-select[formcontrolname="typeId"]'))).toEqual(['Epic', 'Story', 'Bug']);
    expect(texts('h3', dialog())).toEqual(['Fields of this type']);
    expect(await axeViolations(dialog())).toEqual([]);

    // Nothing filled: the required ones say so, nothing is sent.
    await click(dialog().querySelector('button.confirm'));
    expect(texts('mat-error', dialog()).filter(Boolean)).toEqual(['This field is required.', 'This field is required.']);

    await type(dialog().querySelector('input[formcontrolname="summary"]'), 'Migrate the authentication to OIDC');
    await choose(dialog().querySelector('mat-select[aria-label="Application"]'), 'Mail');
    await choose(dialog().querySelector('mat-select[formcontrolname="priority"]'), 'High');
    await click(dialog().querySelector('button.confirm'));
    const sent = await reply(
      (url, method) => url === '/api/v1/projects/PLAT/issues' && method === 'POST',
      {
        type: 'about:blank',
        title: 'Validation failed',
        status: 400,
        code: 'validation_failed',
        errors: [
          { field: 'fields.f-app', code: 'inactive_value' },
          { field: 'summary', code: 'too_long' },
        ],
      },
      400,
    );
    expect(sent.request.headers.get('Idempotency-Key')).toBeTruthy();
    expect(sent.request.body).toEqual({
      typeId: 't-story',
      summary: 'Migrate the authentication to OIDC',
      description: null,
      priority: 'high',
      parentId: null,
      assigneeId: null,
      startDate: null,
      dueDate: null,
      fields: { 'f-app': 'v-mail' },
    });
    // Each error next to its field.
    const errors = texts('mat-error', dialog()).filter(Boolean);
    expect(errors).toContain('The text is too long.');
    expect(errors.length).toBe(2);
    // Refused until the fields are corrected.
    await click(dialog().querySelector('button.confirm'));
    await type(dialog().querySelector('input[formcontrolname="summary"]'), 'Migrate to OIDC');
    await choose(dialog().querySelector('mat-select[aria-label="Application"]'), 'Directory');
    expect(texts('mat-error', dialog()).filter(Boolean)).toEqual([]);

    await click(dialog().querySelector('button.confirm'));
    const again = await reply((url, method) => url === '/api/v1/projects/PLAT/issues' && method === 'POST', ISSUE, 201);
    expect(again.request.body).toEqual(expect.objectContaining({ summary: 'Migrate to OIDC', fields: { 'f-app': 'v-dir' } }));
    expect(again.request.headers.get('Idempotency-Key')).toBe(sent.request.headers.get('Idempotency-Key'));
    expect(await created).toEqual(ISSUE);
    await settle();
    expect(router.url).toBe('/issues/PLAT-142');
  });

  it('fills every kind of field accessibly and sends each as the API takes it', async () => {
    const { reply, dialog, root, text, texts, click, type, choose, open, settle } = await setUp();
    await open({ projectKey: 'PLAT' });
    const fields: IssueFormField[] = [
      field('f-labels', 'Labels', { kind: 'labels' }),
      field('f-erp', 'Module', {
        kind: 'cascading_select',
        values: [{ id: 'v-erp', label: 'ERP', children: [{ id: 'v-hr', label: 'HR', children: [] }] }],
      }),
      field('f-reviewers', 'Reviewers', { kind: 'user', multiple: true }),
      field('f-team', 'Team', { kind: 'group' }),
      field('f-ok', 'Approved', { kind: 'checkbox' }),
      field('f-points', 'Points', { kind: 'number', decimals: 1, unit: 'hours' }),
      field('f-when', 'Deadline', { kind: 'date' }),
      field('f-link', 'Related', { kind: 'issue_link' }),
      field('f-notes', 'Notes', { kind: 'rich_text' }),
    ];
    await reply(isForm, { ...FORM, typeId: 't-story', fields });
    expect(await axeViolations(dialog())).toEqual([]);

    await type(dialog().querySelector('input[formcontrolname="summary"]'), 'Every field');
    // Labels: typed, each ended by Enter.
    const labels = dialog().querySelector('app-field-input .mat-mdc-chip-input') as HTMLInputElement;
    labels.value = 'security';
    labels.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', keyCode: 13, bubbles: true }));
    await settle();
    expect(texts('app-field-input mat-chip-row', dialog()).map((t) => t?.replace('cancel', '').trim())).toEqual(['security']);
    // Cascading list: the detail appears once the first level is chosen.
    expect(dialog().querySelectorAll('app-field-input:nth-of-type(2) mat-select')).toHaveLength(1);
    await choose(dialog().querySelectorAll('app-field-input')[1].querySelector('mat-select'), 'ERP');
    await choose(dialog().querySelectorAll('app-field-input')[1].querySelectorAll('mat-select')[1], 'HR');
    // People and groups: searched in the project.
    const reviewers = dialog().querySelectorAll('app-people-picker input')[1] as HTMLInputElement;
    reviewers.dispatchEvent(new Event('focus'));
    await settle();
    const searched = await reply((url) => url.startsWith('/api/v1/projects/PLAT/people'), [{ id: 'a2', name: 'Camille Martin' }]);
    expect(searched.request.params.get('kind')).toBe('member');
    reviewers.dispatchEvent(new Event('focusin'));
    await settle();
    await click(Array.from(root().querySelectorAll('mat-option')).find((o) => text(o) === 'Camille Martin'));
    const team = dialog().querySelectorAll('app-people-picker input')[2] as HTMLInputElement;
    team.dispatchEvent(new Event('focus'));
    await settle();
    const groups = await reply((url) => url.startsWith('/api/v1/projects/PLAT/people'), [{ id: 'g1', name: 'Platform team' }]);
    expect(groups.request.params.get('kind')).toBe('group');
    team.dispatchEvent(new Event('focusin'));
    await settle();
    await click(Array.from(root().querySelectorAll('mat-option')).find((o) => text(o) === 'Platform team'));
    await click(dialog().querySelector('mat-checkbox input'));
    const inputs = dialog().querySelectorAll('app-field-input input[matinput], app-field-input textarea');
    await type(inputs[0], '2.5');
    await type(inputs[1], '12/04/2026');
    await type(inputs[2], 'PLAT-100');
    await type(inputs[3], 'Some notes');
    expect(text(dialog().querySelector('app-field-input .mat-mdc-form-field-text-suffix'))).toBe('h');

    await click(dialog().querySelector('button.confirm'));
    // The ticket key is resolved to its id first.
    await reply((url) => url.startsWith('/api/v1/issues/PLAT-100'), { ...ISSUE, id: 'i100', key: 'PLAT-100' });
    const sent = await reply((url, method) => method === 'POST' && url.endsWith('/issues'), ISSUE, 201);
    expect(sent.request.body.fields).toEqual({
      'f-labels': ['security'],
      'f-erp': ['v-erp', 'v-hr'],
      'f-reviewers': ['a2'],
      'f-team': 'g1',
      'f-ok': true,
      'f-points': 2.5,
      'f-when': '2026-12-04',
      'f-link': 'i100',
      'f-notes': 'Some notes',
    });
  });

  it('names an unknown ticket next to its field, without creating', async () => {
    const { reply, dialog, texts, click, type, open, http } = await setUp();
    await open({ projectKey: 'PLAT' });
    await reply(isForm, { ...FORM, typeId: 't-story', fields: [field('f-link', 'Related', { kind: 'issue_link' })] });
    await type(dialog().querySelector('input[formcontrolname="summary"]'), 'Linked');
    await type(dialog().querySelector('app-field-input input'), 'PLAT-404');
    await click(dialog().querySelector('button.confirm'));
    await reply((url) => url.startsWith('/api/v1/issues/PLAT-404'), { type: 'about:blank', title: 'Not found', status: 404, code: 'not_found' }, 404);
    expect(texts('mat-error', dialog()).filter(Boolean)).toEqual(['This ticket does not exist.']);
    http.expectNone((r) => r.method === 'POST');
  });

  it('creates a sub-task under its parent with the types it may hold', async () => {
    const { reply, dialog, text, click, type, options, open } = await setUp();
    const { created } = await open({ parent: { id: 'i142', key: 'PLAT-142', typeId: 't-story', projectKey: 'PLAT' } });
    // The API's first type cannot sit under a story: the sub-task is read.
    await reply(isForm, FORM);
    const read = await reply((url) => isForm(url) && url.includes('typeId=t-sub'), { ...FORM, typeId: 't-sub', fields: [] });
    expect(read.request.params.get('typeId')).toBe('t-sub');
    expect(text(dialog().querySelector('h2'))).toBe('New sub-task of PLAT-142');
    expect(await options(dialog().querySelector('mat-select[formcontrolname="typeId"]'))).toEqual(['Sub-task']);
    await type(dialog().querySelector('input[formcontrolname="summary"]'), 'Configure the OIDC client');
    await click(dialog().querySelector('button.confirm'));
    const sent = await reply((url, method) => method === 'POST' && url.endsWith('/issues'), { ...ISSUE, key: 'PLAT-143' }, 201);
    expect(sent.request.body).toEqual(expect.objectContaining({ typeId: 't-sub', parentId: 'i142', fields: {} }));
    expect((await created)?.key).toBe('PLAT-143');
  });

  it('creates a ticket from "My tickets" in a project chosen first', async () => {
    const { reply, dialog, choose, open, options } = await setUp();
    await open({});
    await reply((url) => url.startsWith('/api/v1/nodes?'), {
      items: [
        { ...ISSUE.project, type: 'project', archived: false, confidential: false, description: '', health: 'on_track', objectives: '', owners: [], parents: [], version: 1 },
        { id: 'p2', name: 'Old', key: 'OLD', type: 'project', archived: true, confidential: false, description: '', health: 'on_track', objectives: '', owners: [], parents: [], version: 1 },
        { id: 'p3', veiled: true },
      ],
      nextCursor: null,
    });
    expect(await options(dialog().querySelector('mat-select[formcontrolname="project"]'))).toEqual(['Identity platform (PLAT)']);
    expect(dialog().querySelector('mat-select[formcontrolname="typeId"]')).toBeNull();
    await choose(dialog().querySelector('mat-select[formcontrolname="project"]'), 'Identity platform (PLAT)');
    await reply(isForm, FORM);
    expect(await options(dialog().querySelector('mat-select[formcontrolname="typeId"]'))).toEqual(['Epic', 'Story', 'Bug']);
  });

  it('says so when the member may not create tickets in the project', async () => {
    const { reply, dialog, text, open } = await setUp();
    await open({ projectKey: 'PLAT' });
    await reply(isForm, { type: 'about:blank', title: 'Forbidden', status: 403, code: 'permission.denied' }, 403);
    expect(text(dialog().querySelector('[role="alert"]'))).toBe('You cannot create tickets in this project.');
    expect((dialog().querySelector('button.confirm') as HTMLButtonElement).disabled).toBe(true);
  });

  it('shows a ticket: its type, key, status, description, details and fields', async () => {
    const { harness, reply, root, text, texts } = await setUp();
    await harness.navigateByUrl('/issues/PLAT-142');
    const read = await reply((url) => url.startsWith('/api/v1/issues/PLAT-142'), ISSUE);
    expect(read.request.params.get('expand')).toBe('parent');
    await reply((url) => url === '/api/v1/nodes/p1/my-permissions', ['view', 'create_ticket']);
    await reply(isForm, FORM);
    const page = root().querySelector('app-issue-page')!;
    expect(texts('nav.eyebrow a', page)).toEqual(['Identity platform', 'PLAT-100 Unified authentication']);
    expect(text(page.querySelector('.identity'))).toContain('Story PLAT-142');
    expect(text(page.querySelector('h1'))).toBe('Migrate the authentication to OIDC');
    expect(text(page.querySelector('ui-status-chip'))).toBe('Status: In progress');
    expect(text(page.querySelector('ui-rich-text'))).toBe('Replace the local accounts.');
    expect(page.querySelector('ui-rich-text strong')).not.toBeNull();
    const details = Array.from(page.querySelectorAll('.detail')).map((d) => [text(d.querySelector('dt')), text(d.querySelector('dd'))]);
    expect(details.slice(0, 3)).toEqual([
      ['Assignee', 'Camille Martin'],
      ['Reporter', 'Lucas Bernard'],
      ['Priority', 'keyboard_arrow_up High'],
    ]);
    expect(details.slice(5)).toEqual([
      ['Application', 'Directory'],
      ['Estimate', '8'],
      ['Labels', 'security, directory'],
      ['Approved', 'Yes'],
    ]);
    // A story may hold sub-tasks, and the member creates tickets here.
    expect(text(page.querySelector('button.add-subtask'))).toContain('Add a sub-task');
    expect(await axeViolations(page)).toEqual([]);
  });

  it('offers no sub-task to those who cannot create, and says when the ticket is not found', async () => {
    const { harness, reply, root, text } = await setUp();
    await harness.navigateByUrl('/issues/PLAT-142');
    await reply((url) => url.startsWith('/api/v1/issues/PLAT-142'), { ...ISSUE, descriptionHtml: null, assignee: null, fields: [] });
    await reply((url) => url === '/api/v1/nodes/p1/my-permissions', ['view']);
    const page = root().querySelector('app-issue-page')!;
    expect(page.querySelector('button.add-subtask')).toBeNull();
    expect(text(page.querySelector('.description .empty'))).toBe('No description.');
    expect(text(page.querySelector('.detail dd'))).toBe('Unassigned');

    await harness.navigateByUrl('/issues/PLAT-999');
    await reply((url) => url.startsWith('/api/v1/issues/PLAT-999'), { type: 'about:blank', title: 'Not found', status: 404, code: 'not_found' }, 404);
    expect(root().querySelector('app-issue-page ui-error-state')).not.toBeNull();
  });

  it('lists my tickets a page at a time, and creates one from there', async () => {
    const { harness, reply, root, texts, click, http } = await setUp();
    await harness.navigateByUrl('/tickets');
    await reply((url) => url === '/api/v1/me/issues?limit=50', { items: [summary('PLAT-2', 'Sign-in loops')], nextCursor: 'c1' });
    const page = root().querySelector('app-my-tickets-page')!;
    expect(texts('tbody tr td:nth-child(2)', page)).toEqual(['Sign-in loops']);
    expect(page.querySelector('tbody a')?.getAttribute('href')).toBe('/issues/PLAT-2');
    expect(await axeViolations(page)).toEqual([]);
    await click(page.querySelector('button.more'));
    await reply((url) => url === '/api/v1/me/issues?limit=50&cursor=c1', { items: [summary('PLAT-1', 'Story')], nextCursor: null });
    expect(texts('tbody tr td:nth-child(2)', page)).toEqual(['Sign-in loops', 'Story']);
    expect(page.querySelector('button.more')).toBeNull();

    await click(page.querySelector('button.create'));
    await reply((url) => url.startsWith('/api/v1/nodes?'), { items: [], nextCursor: null });
    expect(root().querySelector('mat-dialog-container mat-select[formcontrolname="project"]')).not.toBeNull();
    http.verify();
  });

  it('says when I have no ticket yet', async () => {
    const { harness, reply, root, text } = await setUp();
    await harness.navigateByUrl('/tickets');
    await reply((url) => url.startsWith('/api/v1/me/issues'), { items: [], nextCursor: null });
    expect(text(root().querySelector('app-my-tickets-page ui-empty-state'))).toContain('No ticket yet');
  });
});
