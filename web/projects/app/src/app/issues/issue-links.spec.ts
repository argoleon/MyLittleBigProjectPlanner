/**
 * Tests of sub-tasks, links and parents in the application (L05-US03): the
 * lines of the card, then with the API mocked at the HTTP level and the
 * real translations: the card on the ticket's page, a link added after a
 * search and removed after confirming, a link refused, the parent changed
 * among the allowed tickets, nothing offered to readers, and accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { AuthService, Issue, IssueRelated, IssueTypeRef, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { IssuePage } from './issue-page';
import { relationLines } from './issue-relations';

const SESSION = {
  account: { id: 'a1', email: 'lucas@acme.example', displayName: 'Lucas Bernard', locale: null, tenantRole: 'member' },
  stage: 'complete',
  csrfToken: 'csrf',
  expiresAt: '2026-11-20T09:00:00Z',
};

const type = (id: string, label: string, color: string, parentIds: string[], isSubtask = false): IssueTypeRef => ({
  id,
  key: label.toLowerCase(),
  label,
  labelTranslations: { en: label },
  icon: 'bookmark',
  color,
  isSubtask,
  parentIds,
});
const EPIC = type('t-epic', 'Epic', 'epic', []);
const STORY = type('t-story', 'Story', 'story', ['t-epic']);
const SUBTASK = type('t-sub', 'Sub-task', 'task', ['t-story'], true);

const PLAT = { id: 'p1', key: 'PLAT', name: 'Identity platform' };
const CRM = { id: 'p2', key: 'CRM', name: 'Customer relations' };
const TODO = { id: 's1', key: 'todo', label: 'To do', category: 'todo' as const };
const DONE = { id: 's3', key: 'done', label: 'Done', category: 'done' as const };

const related = (id: string, key: string, summary: string, rest: Partial<IssueRelated> = {}): IssueRelated => ({
  id,
  key,
  summary,
  project: PLAT,
  type: STORY,
  status: TODO,
  ...rest,
});

const ISSUE: Issue = {
  id: 'i142',
  key: 'PLAT-142',
  project: PLAT,
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
  fields: [],
  parent: { id: 'i100', key: 'PLAT-100', summary: 'Unified sign-in' },
  parentId: 'i100',
  children: [related('i143', 'PLAT-143', 'Configure the OIDC client', { type: SUBTASK, status: DONE })],
  links: [
    { id: 'l1', relation: 'depends_on', issue: related('i120', 'PLAT-120', 'Directory scoping', { status: DONE }) },
    { id: 'l2', relation: 'blocks', issue: related('i224', 'CRM-224', 'Customer portal forms', { project: CRM }) },
  ],
  createdAt: '2026-11-06T09:12:00Z',
  updatedAt: '2026-11-06T18:20:00Z',
  version: 4,
};

describe('the lines of the sub-tasks and links card', () => {
  it('list the sub-tasks first, then the links as they read from the ticket', () => {
    expect(relationLines(ISSUE).map((l) => [l.relation, l.issue.key, l.icon, l.link?.id ?? null])).toEqual([
      ['subtask', 'PLAT-143', 'subdirectory_arrow_right', null],
      ['depends_on', 'PLAT-120', 'link', 'l1'],
      ['blocks', 'CRM-224', 'block', 'l2'],
    ]);
    expect(relationLines({ ...ISSUE, children: undefined, links: undefined })).toEqual([]);
  });
});

describe('sub-tasks, links and parents on the ticket page', () => {
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
    const isTicket = (url: string, method: string) => url.startsWith('/api/v1/issues/PLAT-142') && method === 'GET';
    const loaded = TestBed.inject(AuthService).load();
    await reply((url) => url === '/api/v1/auth/session', SESSION);
    await loaded;
    const harness = await RouterTestingHarness.create();
    await harness.navigateByUrl('/issues/PLAT-142');
    await reply(isTicket, ISSUE);
    await reply((url) => url === '/api/v1/nodes/p1/my-permissions', permissions);
    const root = () => document.body;
    const page = () => root().querySelector('app-issue-page')!;
    const card = () => page().querySelector('app-issue-relations')!;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const texts = (selector: string, within: Element = root()) => Array.from(within.querySelectorAll(selector)).map((e) => text(e));
    const click = async (element: Element | null | undefined) => {
      (element as HTMLElement).click();
      await settle();
    };
    const dialog = () => Array.from(root().querySelectorAll('mat-dialog-container')).at(-1)!;
    /** Types `value` in the ticket picker of the dialog and picks `key` from what is found. */
    const pick = async (value: string, found: IssueRelated[], key: string) => {
      const input = dialog().querySelector('app-issue-picker input') as HTMLInputElement;
      input.focus();
      input.value = value;
      input.dispatchEvent(new Event('input'));
      await new Promise((resolve) => setTimeout(resolve, 300));
      await settle();
      const searched = await reply((url) => url.startsWith('/api/v1/issues?'), found);
      await click(Array.from(root().querySelectorAll('mat-option')).find((o) => text(o)?.startsWith(key)));
      return searched;
    };
    return { http, reply, settle, root, page, card, text, texts, click, dialog, pick, isTicket };
  }

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it('shows the sub-tasks and links, each with its relation, key, project and status', async () => {
    const { card, texts, text } = await openTicket();
    expect(text(card().querySelector('h2'))).toBe('Sub-tasks and links');
    expect(texts('.line .relation', card())).toEqual(['Sub-task', 'Depends on', 'Blocks']);
    expect(texts('.line .key', card())).toEqual(['PLAT-143', 'PLAT-120', 'CRM-224']);
    expect(card().querySelector('.line .key')?.getAttribute('href')).toBe('/issues/PLAT-143');
    expect(text(card().querySelectorAll('.line .summary')[2])).toBe('Customer portal forms · Customer relations');
    expect(texts('.line ui-status-chip', card())).toEqual(['Status: Done', 'Status: Done', 'Status: To do']);
    // Links can be removed, sub-tasks not.
    expect(Array.from(card().querySelectorAll('button.remove')).map((b) => b.getAttribute('aria-label'))).toEqual([
      'Remove the link with PLAT-120',
      'Remove the link with CRM-224',
    ]);
    expect(await axeViolations(card())).toEqual([]);
  });

  it('links a ticket found by its summary, then reads the ticket again', async () => {
    const { card, dialog, click, reply, pick, isTicket, text } = await openTicket();
    await click(card().querySelector('button.add-link'));
    expect(text(dialog().querySelector('h2'))).toBe('Link PLAT-142 to a ticket');
    await click(dialog().querySelector('mat-select .mat-mdc-select-trigger'));
    await click(Array.from(document.querySelectorAll('mat-option')).find((o) => text(o) === 'Blocked by'));
    const searched = await pick(
      'portal',
      [related('i142', 'PLAT-142', 'Itself'), related('i300', 'CRM-300', 'Portal sign-in', { project: CRM })],
      'CRM-300',
    );
    expect(searched.request.params.get('search')).toBe('portal');
    expect(await axeViolations(dialog())).toEqual([]);
    await click(dialog().querySelector('button.confirm'));
    const sent = await reply((url, method) => url === '/api/v1/issues/i142/links' && method === 'POST', {
      id: 'l3',
      relation: 'blocked_by',
      issue: related('i300', 'CRM-300', 'Portal sign-in', { project: CRM }),
    }, 201);
    expect(sent.request.body).toEqual({ relation: 'blocked_by', issue: 'i300' });
    await reply(isTicket, ISSUE);
  });

  it('says why a link is refused, next to the ticket or above', async () => {
    const { card, dialog, click, reply, pick, texts, text } = await openTicket();
    await click(card().querySelector('button.add-link'));
    await pick('PLAT-120', [related('i120', 'PLAT-120', 'Directory scoping')], 'PLAT-120');
    await click(dialog().querySelector('button.confirm'));
    await reply(
      (url, method) => method === 'POST' && url.endsWith('/links'),
      { type: 'about:blank', title: 'Conflict', status: 409, code: 'issue_link.exists' },
      409,
    );
    expect(text(dialog().querySelector('[role="alert"]'))).toBe('The two tickets already have this link.');
    await click(dialog().querySelector('button.confirm'));
    await reply(
      (url, method) => method === 'POST' && url.endsWith('/links'),
      { type: 'about:blank', title: 'Invalid', status: 400, code: 'validation_failed', errors: [{ field: 'issue', code: 'not_found' }] },
      400,
    );
    expect(texts('app-issue-picker .error[role="alert"]', dialog())).toEqual(['Not found.']);
  });

  it('removes a link once confirmed', async () => {
    const { card, root, click, reply, isTicket, text } = await openTicket();
    await click(card().querySelectorAll('button.remove')[1]);
    const confirm = root().querySelector('ui-confirm-dialog')!;
    expect(text(confirm.querySelector('p'))).toBe('The link with CRM-224 is removed from both tickets.');
    await click(Array.from(confirm.querySelectorAll('button')).find((b) => text(b) === 'Remove'));
    await reply((url, method) => url === '/api/v1/issues/i142/links/l2' && method === 'DELETE', null, 204);
    await reply(isTicket, { ...ISSUE, links: ISSUE.links!.slice(0, 1) });
    expect(card().querySelectorAll('.line')).toHaveLength(2);
  });

  it('changes the parent among the tickets its type may sit under', async () => {
    const { page, dialog, click, reply, pick, isTicket, text, root } = await openTicket();
    await click(page().querySelector('button.change-parent'));
    expect(text(dialog().querySelector('h2'))).toBe('Parent of PLAT-142');
    const searched = await pick(
      'security',
      [
        related('i500', 'PLAT-500', 'Security', { type: EPIC }),
        related('i501', 'PLAT-501', 'Security review', { type: STORY }),
        related('i100', 'PLAT-100', 'Unified sign-in', { type: EPIC }),
      ],
      'PLAT-500',
    );
    // Only epics, in the project, the current parent left out.
    expect(searched.request.params.get('projectKey')).toBe('PLAT');
    expect(root().querySelectorAll('mat-option')).toHaveLength(0);
    await click(dialog().querySelector('button.confirm'));
    const sent = await reply((url, method) => url === '/api/v1/issues/i142/parent' && method === 'PUT', { ...ISSUE, parentId: 'i500', version: 5 });
    expect(sent.request.headers.get('If-Match')).toBe('"4"');
    expect(sent.request.body).toEqual({ parentId: 'i500' });
    await reply(isTicket, ISSUE);
  });

  it('puts a ticket at the top, never a sub-task', async () => {
    const { page, dialog, click, reply } = await openTicket();
    await click(page().querySelector('button.change-parent'));
    await click(dialog().querySelector('mat-checkbox input'));
    expect(dialog().querySelector('app-issue-picker')).toBeNull();
    await click(dialog().querySelector('button.confirm'));
    const sent = await reply((url, method) => method === 'PUT' && url.endsWith('/parent'), { ...ISSUE, parentId: null });
    expect(sent.request.body).toEqual({ parentId: null });
  });

  it('offers no link, removal nor parent change to those who only view', async () => {
    const { page, card } = await openTicket(['view']);
    expect(card().querySelector('button.add-link')).toBeNull();
    expect(card().querySelector('button.remove')).toBeNull();
    expect(page().querySelector('button.change-parent')).toBeNull();
    expect(card().querySelectorAll('.line')).toHaveLength(3);
  });
});
