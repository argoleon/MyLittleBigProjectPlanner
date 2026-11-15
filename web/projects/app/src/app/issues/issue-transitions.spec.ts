/**
 * Tests of status changes on the ticket page (L05-US09), with the API
 * mocked at the HTTP level and the real translations: the transitions
 * offered as a menu and the first as a button, one run at once with the
 * version read, one that takes fields shown first with its refusals next
 * to them, a conflict offering to reload, a refusal said, readers seeing
 * the status only, and accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { AuthService, Issue, IssueTransition, IssueTypeRef, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

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
  parentIds: [],
};

const TODO = { id: 's1', key: 'todo', label: 'To do', category: 'todo' as const };
const PROGRESS = { id: 's2', key: 'in_progress', label: 'In progress', category: 'in_progress' as const };
const REVIEW = { id: 's3', key: 'in_review', label: 'In review', category: 'in_progress' as const };

const ISSUE: Issue = {
  id: 'i142',
  key: 'PLAT-142',
  project: { id: 'p1', key: 'PLAT', name: 'Identity platform' },
  type: STORY,
  status: TODO,
  summary: 'Migrate the authentication to OIDC',
  description: null,
  descriptionHtml: null,
  priority: 'medium',
  reporter: null,
  assignee: null,
  startDate: null,
  dueDate: null,
  fields: [{ fieldId: 'f-points', key: 'points', label: 'Points', kind: 'number', value: 3, display: null, references: [] }],
  parentId: null,
  children: [],
  links: [],
  createdAt: '2026-11-06T09:12:00Z',
  updatedAt: '2026-11-06T18:20:00Z',
  version: 4,
};

const START: IssueTransition = { id: 'tr-start', name: 'Start', toStatus: PROGRESS, fields: [] };
const REVIEW_IT: IssueTransition = {
  id: 'tr-review',
  name: 'Send to review',
  toStatus: REVIEW,
  fields: [
    { fieldId: 'f-points', key: 'points', label: 'Points', kind: 'number', mode: 'visible' },
    { fieldId: 'f-remaining', key: 'remaining', label: 'Remaining work', kind: 'number', mode: 'required', unit: 'hours' },
  ],
};

describe('status changes on the ticket page', () => {
  async function openTicket(permissions: string[] = ['view', 'edit'], transitions: IssueTransition[] = [START, REVIEW_IT]) {
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
    const isList = (url: string, method: string) => url === '/api/v1/issues/i142/transitions' && method === 'GET';
    const isRun = (url: string, method: string) => url === '/api/v1/issues/i142/transitions' && method === 'POST';
    const loaded = TestBed.inject(AuthService).load();
    await reply((url) => url === '/api/v1/auth/session', SESSION);
    await loaded;
    const harness = await RouterTestingHarness.create();
    await harness.navigateByUrl('/issues/PLAT-142');
    await reply((url, method) => url.startsWith('/api/v1/issues/PLAT-142') && method === 'GET', ISSUE);
    await reply((url) => url === '/api/v1/nodes/p1/my-permissions', permissions);
    if (permissions.includes('edit')) await reply(isList, transitions);
    const root = () => document.body;
    const page = () => root().querySelector('app-issue-page')!;
    const status = () => page().querySelector('app-issue-status')!;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const texts = (selector: string, within: Element = root()) => Array.from(within.querySelectorAll(selector)).map((e) => text(e));
    const click = async (element: Element | null | undefined) => {
      (element as HTMLElement).click();
      await settle();
    };
    const dialog = () => root().querySelector('mat-dialog-container')!;
    return { http, reply, settle, root, page, status, text, texts, click, dialog, isList, isRun };
  }

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it('offers the transitions as a menu, the first one as a button', async () => {
    const { status, root, click, texts, text } = await openTicket();
    const menu = status().querySelector('button.status-menu')!;
    expect(menu.getAttribute('aria-label')).toBe('Status: To do. Change it');
    expect(text(status().querySelector('button.first-transition'))).toBe('Start');
    expect(status().querySelector('ui-status-chip')).toBeNull();
    await click(menu);
    expect(texts('button.transition', root())).toEqual(['Start → In progress', 'Send to review → In review']);
    expect(await axeViolations(status().parentElement!)).toEqual([]);
  });

  it('runs a transition that takes no field at once, with the version read', async () => {
    const { status, page, click, reply, isRun, isList, text } = await openTicket();
    await click(status().querySelector('button.first-transition'));
    const sent = await reply(isRun, { ...ISSUE, status: PROGRESS, assignee: { id: 'a1', name: 'Lucas Bernard' }, version: 5 });
    expect(sent.request.headers.get('If-Match')).toBe('"4"');
    expect(sent.request.body).toEqual({ transitionId: 'tr-start' });
    // The new status shown; the transitions read again for it.
    await reply(isList, [REVIEW_IT]);
    expect(text(status().querySelector('button.status-menu'))).toContain('In progress');
    expect(text(status().querySelector('button.first-transition'))).toBe('Send to review');
    expect(text(page().querySelectorAll('.detail dd')[0])).toBe('Lucas Bernard');
  });

  it('shows the fields a transition takes first, its refusals next to them', async () => {
    const { status, root, dialog, click, reply, isRun, texts, text, settle } = await openTicket();
    await click(status().querySelector('button.status-menu'));
    await click(Array.from(root().querySelectorAll('button.transition')).find((b) => text(b)?.startsWith('Send to review')));
    expect(text(dialog().querySelector('h2'))).toBe('Send to review');
    expect(text(dialog().querySelector('.note'))).toBe('The ticket goes to “In review”.');
    const inputs = dialog().querySelectorAll<HTMLInputElement>('app-field-input input');
    expect(inputs[0].value).toBe('3');
    expect(await axeViolations(dialog())).toEqual([]);
    // The remaining work is required: said before anything is sent.
    await click(dialog().querySelector('button.confirm'));
    expect(texts('mat-error', dialog()).filter(Boolean)).toEqual(['This field is required.']);
    inputs[1].value = '4';
    inputs[1].dispatchEvent(new Event('input'));
    await settle();
    await click(dialog().querySelector('button.confirm'));
    const refused = await reply(
      isRun,
      { type: 'about:blank', title: 'Invalid', status: 400, code: 'validation_failed', errors: [{ field: 'fields.f-remaining', code: 'out_of_range' }] },
      400,
    );
    expect(refused.request.body).toEqual({ transitionId: 'tr-review', fields: { 'f-remaining': 4 } });
    expect(texts('mat-error', dialog()).filter(Boolean)).toEqual(['The value is out of the allowed range.']);
    inputs[1].value = '5';
    inputs[1].dispatchEvent(new Event('input'));
    await settle();
    await click(dialog().querySelector('button.confirm'));
    const sent = await reply(isRun, { ...ISSUE, status: REVIEW, version: 5 });
    expect(sent.request.body).toEqual({ transitionId: 'tr-review', fields: { 'f-remaining': 5 } });
  });

  it('says when someone changed the ticket first, and reads it again; or why it is refused', async () => {
    const { status, click, reply, isRun, text } = await openTicket();
    await click(status().querySelector('button.first-transition'));
    await reply(isRun, { type: 'about:blank', title: 'Precondition failed', status: 412, code: 'precondition_failed' }, 412);
    expect(text(status().querySelector('.conflict'))).toContain('Someone changed this ticket since you opened it');
    await click(status().querySelector('button.reload'));
    await reply((url, method) => url.startsWith('/api/v1/issues/PLAT-142') && method === 'GET', { ...ISSUE, version: 5 });

    TestBed.inject(MatDialog).closeAll();
    TestBed.resetTestingModule();
    const fresh = await openTicket(['view', 'edit'], [START]);
    await fresh.click(fresh.status().querySelector('button.first-transition'));
    await fresh.reply(fresh.isRun, { type: 'about:blank', title: 'Forbidden', status: 403, code: 'permission.denied' }, 403);
    expect(fresh.status().querySelector('[role="alert"]')).not.toBeNull();
  });

  it('shows the status only to those who view, or when no transition is offered', async () => {
    const reader = await openTicket(['view']);
    expect(reader.text(reader.status().querySelector('ui-status-chip'))).toBe('Status: To do');
    expect(reader.status().querySelector('button')).toBeNull();
    reader.http.expectNone((r) => r.url.endsWith('/transitions'));
    TestBed.resetTestingModule();
    const stuck = await openTicket(['view', 'edit'], []);
    expect(stuck.status().querySelector('ui-status-chip')).not.toBeNull();
  });
});
