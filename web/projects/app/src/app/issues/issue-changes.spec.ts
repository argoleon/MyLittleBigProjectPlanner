/**
 * Tests of changing tickets in the application (L05-US02): values read
 * back into the form, the targets offered when mapping a move, then with
 * the API mocked at the HTTP level and the real translations: editing from
 * the ticket's page (only what changed is sent, with the version read; a
 * conflict offers to reload), moving to another project with its mappings
 * and the values dropped, those who may not, and accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { Router, provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { AuthService, Issue, IssueEditForm, IssueFormField, IssueMovePlan, IssueTypeRef, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { rawOf } from './issue-fields';
import { statusTargets, typeTargets } from './issue-move-dialog';
import { IssuePage } from './issue-page';

const SESSION = {
  account: { id: 'a1', email: 'lucas@acme.example', displayName: 'Lucas Bernard', locale: null, tenantRole: 'member' },
  stage: 'complete',
  csrfToken: 'csrf',
  expiresAt: '2026-11-20T09:00:00Z',
};

const STORY: IssueTypeRef = {
  id: 't-story',
  key: 'story',
  label: 'Story',
  labelTranslations: { en: 'Story' },
  icon: 'bookmark',
  color: 'story',
  isSubtask: false,
  parentIds: ['t-epic'],
};

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
const REVIEWERS = field('f-rev', 'Reviewers', { kind: 'user', multiple: true });

const ISSUE: Issue = {
  id: 'i142',
  key: 'PLAT-142',
  project: { id: 'p1', key: 'PLAT', name: 'Identity platform' },
  type: STORY,
  status: { id: 's1', key: 'todo', label: 'To do', category: 'todo' },
  summary: 'Migrate the authentication to OIDC',
  description: null,
  descriptionHtml: null,
  priority: 'medium',
  reporter: { id: 'a1', name: 'Lucas Bernard' },
  assignee: { id: 'a2', name: 'Camille Martin' },
  startDate: '2026-11-09',
  dueDate: null,
  fields: [
    { fieldId: 'f-app', key: 'application', label: 'Application', kind: 'single_select', value: 'v-dir', display: 'Directory', references: [] },
    {
      fieldId: 'f-rev',
      key: 'reviewers',
      label: 'Reviewers',
      kind: 'user',
      value: ['a3'],
      display: 'Hugo Leroy',
      references: [{ id: 'a3', name: 'Hugo Leroy' }],
    },
  ],
  parentId: null,
  createdAt: '2026-11-06T09:12:00Z',
  updatedAt: '2026-11-06T18:20:00Z',
  version: 3,
};

const EDIT_FORM: IssueEditForm = { fields: [APPLICATION, REVIEWERS] };

const PLAN: IssueMovePlan = {
  project: { id: 'p2', key: 'CRM', name: 'Customer relations' },
  sameConfiguration: false,
  tickets: [
    { id: 'i142', key: 'PLAT-142', summary: 'Migrate the authentication to OIDC' },
    { id: 'i143', key: 'PLAT-143', summary: 'Configure the client' },
  ],
  parentCleared: { id: 'i100', key: 'PLAT-100', summary: 'Unified sign-in' },
  targets: [
    { id: 'c-story', label: 'User story', isSubtask: false, parentIds: [], statusIds: ['c-open', 'c-closed'] },
    { id: 'c-incident', label: 'Incident', isSubtask: false, parentIds: [], statusIds: ['c-new'] },
    { id: 'c-sub', label: 'Step', isSubtask: true, parentIds: ['c-story'], statusIds: ['c-open', 'c-closed'] },
  ],
  statusTargets: [
    { id: 'c-open', label: 'Open', category: 'todo' },
    { id: 'c-closed', label: 'Closed', category: 'done' },
    { id: 'c-new', label: 'New', category: 'todo' },
  ],
  types: [
    { id: 't-story', key: 'story', label: 'Story', isSubtask: false, suggestedId: 'c-story' },
    { id: 't-sub', key: 'subtask', label: 'Sub-task', isSubtask: true, suggestedId: 'c-sub' },
  ],
  statuses: [{ id: 's1', key: 'todo', label: 'To do', category: 'todo', typeIds: ['t-story', 't-sub'], suggestedId: 'c-open' }],
  droppedFields: [{ fieldId: 'f-app', label: 'Application' }],
  problems: [],
};

describe('values read back into the form', () => {
  it('turn what the ticket holds into what the controls take', () => {
    const held = (kind: IssueFormField['kind'], value: unknown, references: { id: string; name: string }[] = []) => ({
      fieldId: 'f',
      key: 'f',
      kind,
      value,
      display: null,
      references,
    });
    expect(rawOf(APPLICATION, undefined)).toBeNull();
    expect(rawOf(REVIEWERS, undefined)).toEqual([]);
    expect(rawOf(APPLICATION, held('single_select', 'v-dir'))).toBe('v-dir');
    expect(rawOf(field('f', 'F', { kind: 'date' }), held('date', '2026-11-09'))).toEqual(new Date(2026, 10, 9));
    expect(rawOf(field('f', 'F', { kind: 'date_time' }), held('date_time', new Date(2026, 10, 9, 10, 30).toISOString()))).toBe(
      '2026-11-09T10:30',
    );
    expect(rawOf(REVIEWERS, held('user', ['a3'], [{ id: 'a3', name: 'Hugo' }]))).toEqual([{ id: 'a3', name: 'Hugo' }]);
    expect(rawOf(field('f', 'F', { kind: 'issue_link' }), held('issue_link', 'i1', [{ id: 'i1', name: 'PLAT-1' }]))).toBe('PLAT-1');
    expect(rawOf(field('f', 'F', { kind: 'issue_link' }), held('issue_link', 'i1'))).toBeNull();
    expect(rawOf(field('f', 'F', { kind: 'labels' }), held('labels', ['a', 'b']))).toEqual(['a', 'b']);
    expect(rawOf(field('f', 'F', { kind: 'number' }), held('number', 8))).toBe(8);
  });
});

describe('the targets of a move', () => {
  it('offer types that are no sub-task to the ticket, allowed sub-task types to its sub-tasks', () => {
    expect(typeTargets(PLAN, PLAN.types[0], null).map((t) => t.label)).toEqual(['User story', 'Incident']);
    expect(typeTargets(PLAN, PLAN.types[1], 'c-story').map((t) => t.label)).toEqual(['Step']);
    expect(typeTargets(PLAN, PLAN.types[1], 'c-incident')).toEqual([]);
  });

  it('offer the statuses of the workflows of the new types', () => {
    expect(statusTargets(PLAN, PLAN.statuses[0], { 't-story': 'c-story', 't-sub': 'c-sub' }).map((s) => s.label)).toEqual([
      'Open',
      'Closed',
    ]);
    expect(statusTargets(PLAN, PLAN.statuses[0], { 't-story': 'c-incident', 't-sub': 'c-sub' }).map((s) => s.label)).toEqual([
      'Open',
      'Closed',
      'New',
    ]);
  });
});

describe('changing a ticket from its page', () => {
  async function openTicket(permissions: string[] = ['view', 'edit']) {
    localStorage.clear();
    TestBed.configureTestingModule({
      providers: [
        provideRouter([{ path: 'issues/:key', component: IssuePage }], withComponentInputBinding()),
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
    await reply((url) => url === '/api/v1/auth/session', SESSION);
    await loaded;
    const harness = await RouterTestingHarness.create();
    await harness.navigateByUrl('/issues/PLAT-142');
    await reply((url, method) => url.startsWith('/api/v1/issues/PLAT-142') && method === 'GET', ISSUE);
    await reply((url) => url === '/api/v1/nodes/p1/my-permissions', permissions);
    const root = () => document.body;
    const page = () => root().querySelector('app-issue-page')!;
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
    const choose = async (select: Element | null, label: string) => {
      await click(select?.querySelector('.mat-mdc-select-trigger'));
      await click(Array.from(root().querySelectorAll('mat-option')).reverse().find((o) => text(o) === label));
    };
    return { http, harness, reply, settle, root, page, text, texts, click, dialog, type, choose };
  }

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it('edits a ticket, sending only what changed with the version read', async () => {
    const { page, dialog, click, reply, type, choose, text, texts, settle } = await openTicket();
    await click(page().querySelector('button.edit'));
    await reply((url) => url === '/api/v1/issues/i142/edit-form', EDIT_FORM);
    expect(text(dialog().querySelector('h2'))).toBe('Edit PLAT-142');
    // The values held, in their controls.
    expect((dialog().querySelector('input[formcontrolname="summary"]') as HTMLInputElement).value).toBe(ISSUE.summary);
    expect(text(dialog().querySelector('mat-select[aria-label="Application"] .mat-mdc-select-value'))).toBe('Directory');
    expect(texts('app-people-picker mat-chip-row', dialog()).map((t) => t?.replace('cancel', '').trim())).toEqual([
      'Camille Martin',
      'Hugo Leroy',
    ]);
    expect(await axeViolations(dialog())).toEqual([]);

    await type(dialog().querySelector('input[formcontrolname="summary"]'), 'Migrate to OIDC');
    await choose(dialog().querySelector('mat-select[aria-label="Application"]'), 'Mail');
    await click(dialog().querySelector('button.confirm'));
    const saved = await reply((url, method) => url === '/api/v1/issues/i142' && method === 'PATCH', {
      ...ISSUE,
      summary: 'Migrate to OIDC',
      version: 4,
    });
    expect(saved.request.headers.get('If-Match')).toBe('"3"');
    expect(saved.request.body).toEqual({ summary: 'Migrate to OIDC', fields: { 'f-app': 'v-mail' } });
    // The page shows the ticket saved, once the dialog has closed.
    for (let i = 0; i < 50 && text(page().querySelector('h1')) !== 'Migrate to OIDC'; i++) await settle();
    expect(text(page().querySelector('h1'))).toBe('Migrate to OIDC');
    expect(TestBed.inject(MatDialog).openDialogs).toEqual([]);
  });

  it('says when someone saved first, and reloads the ticket', async () => {
    const { page, dialog, click, reply, type, text } = await openTicket();
    await click(page().querySelector('button.edit'));
    await reply((url) => url.endsWith('/edit-form'), EDIT_FORM);
    await type(dialog().querySelector('input[formcontrolname="summary"]'), 'Mine');
    await click(dialog().querySelector('button.confirm'));
    await reply(
      (url, method) => method === 'PATCH',
      { type: 'about:blank', title: 'Precondition failed', status: 412, code: 'precondition_failed' },
      412,
    );
    expect(text(dialog().querySelector('.conflict p'))).toContain('Someone changed this ticket since you opened it');
    expect((dialog().querySelector('button.confirm') as HTMLButtonElement).disabled).toBe(true);
    await click(dialog().querySelector('button.reload'));
    await reply((url, method) => url.startsWith('/api/v1/issues/PLAT-142') && method === 'GET', { ...ISSUE, summary: 'Theirs', version: 4 });
    expect(text(page().querySelector('h1'))).toBe('Theirs');
  });

  it('moves a ticket with its sub-tasks, mapping what the target lacks, then opens it', async () => {
    const { page, dialog, click, reply, choose, text, texts } = await openTicket(['view', 'edit', 'create_ticket']);
    await reply((url) => url.startsWith('/api/v1/projects/PLAT/issue-form'), { project: ISSUE.project, types: [STORY], typeId: 't-story', fields: [] });
    await click(page().querySelector('button.move'));
    await reply((url) => url.startsWith('/api/v1/nodes?'), {
      items: [
        { id: 'p1', type: 'project', name: 'Identity platform', key: 'PLAT', archived: false },
        { id: 'p2', type: 'project', name: 'Customer relations', key: 'CRM', archived: false },
      ],
      nextCursor: null,
    });
    expect(text(dialog().querySelector('h2'))).toBe('Move PLAT-142 to another project');
    await choose(dialog().querySelector('mat-select[aria-label="Target project"]'), 'Customer relations (CRM)');
    const first = await reply((url, method) => url === '/api/v1/issues/i142/move-plan' && method === 'POST', PLAN);
    expect(first.request.body).toEqual({ projectKey: 'CRM', typeMappings: [], statusMappings: [] });
    // The suggestions fill the mappings: planned again with them.
    const second = await reply((url) => url.endsWith('/move-plan'), PLAN);
    expect(second.request.body.typeMappings).toEqual([
      { fromId: 't-story', toId: 'c-story' },
      { fromId: 't-sub', toId: 'c-sub' },
    ]);
    expect(second.request.body.statusMappings).toEqual([{ fromId: 's1', toId: 'c-open' }]);
    expect(texts('.tickets li', dialog())).toEqual(['PLAT-142 Migrate the authentication to OIDC', 'PLAT-143 Configure the client']);
    expect(text(dialog().querySelector('.dropped'))).toContain('Application');
    expect(text(dialog().querySelector('mat-select.type-mapping .mat-mdc-select-value'))).toBe('User story');
    expect(await axeViolations(dialog())).toEqual([]);

    // A problem the move would have: shown, the move held back.
    await choose(dialog().querySelector('mat-select.type-mapping'), 'Incident');
    await reply((url) => url.endsWith('/move-plan'), {
      ...PLAN,
      problems: [{ field: 'typeMappings.t-sub', code: 'not_allowed' }],
    });
    expect(texts('mat-hint.field-error', dialog())).toEqual(['Not allowed here.']);
    expect((dialog().querySelector('button.confirm') as HTMLButtonElement).disabled).toBe(true);
    await choose(dialog().querySelector('mat-select.type-mapping'), 'User story');
    await reply((url) => url.endsWith('/move-plan'), PLAN);

    await click(dialog().querySelector('button.confirm'));
    const moved = await reply((url, method) => url === '/api/v1/issues/i142/move' && method === 'POST', {
      ...ISSUE,
      key: 'CRM-7',
      project: PLAN.project,
      version: 4,
    });
    expect(moved.request.headers.get('If-Match')).toBe('"3"');
    expect(moved.request.body).toEqual({
      projectKey: 'CRM',
      typeMappings: [
        { fromId: 't-story', toId: 'c-story' },
        { fromId: 't-sub', toId: 'c-sub' },
      ],
      statusMappings: [{ fromId: 's1', toId: 'c-open' }],
    });
    // Its page, once the dialog has closed.
    const router = TestBed.inject(Router);
    for (let i = 0; i < 50 && router.url !== '/issues/CRM-7'; i++) await new Promise((resolve) => setTimeout(resolve, 20));
    expect(router.url).toBe('/issues/CRM-7');
  });

  it('offers neither edit nor move to those who only view the project', async () => {
    const { page } = await openTicket(['view']);
    expect(page().querySelector('button.edit')).toBeNull();
    expect(page().querySelector('button.move')).toBeNull();
  });
});
