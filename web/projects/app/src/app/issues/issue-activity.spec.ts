/**
 * Tests of the Activity tab of the ticket page (L05-US06), with the API
 * mocked at the HTTP level and the real translations: the tabs (Activity
 * first, Comments with their number), entries newest first as sentences
 * with their date and "via MCP", a description's before and after on
 * demand, older entries page by page, the history read again when a
 * comment changes, its states, and accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { By } from '@angular/platform-browser';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { ActivityEntry, ActivityPage, AuthService, Issue, IssueComment, LocaleService, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { IssueComments } from './issue-comments';
import { IssuePage } from './issue-page';

const SESSION = {
  account: { id: 'a1', email: 'lucas@acme.example', displayName: 'Lucas Bernard', locale: null, tenantRole: 'member' },
  stage: 'complete',
  csrfToken: 'csrf',
  expiresAt: '2026-11-20T09:00:00Z',
};

const ISSUE: Issue = {
  id: 'i142',
  key: 'PLAT-142',
  project: { id: 'p1', key: 'PLAT', name: 'Identity platform' },
  type: { id: 't', key: 'story', label: 'Story', labelTranslations: { en: 'Story' }, icon: 'bookmark', color: 'story', isSubtask: false, parentIds: [] },
  status: { id: 's2', key: 'in_progress', label: 'In progress', category: 'in_progress' },
  summary: 'Migrate the authentication to OIDC',
  description: null,
  descriptionHtml: null,
  priority: 'medium',
  reporter: null,
  assignee: null,
  startDate: null,
  dueDate: null,
  fields: [],
  parentId: null,
  children: [],
  links: [],
  createdAt: '2026-11-02T08:00:00Z',
  updatedAt: '2026-11-06T18:20:00Z',
  version: 4,
};

const entry = (id: string, rest: Partial<ActivityEntry>): ActivityEntry => ({
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

const ENTRIES: ActivityEntry[] = [
  entry('e4', {
    action: 'updated',
    field: 'custom',
    fieldLabel: 'Application',
    old: { kind: 'text', text: 'Directory', html: null },
    new: { kind: 'text', text: 'Mail', html: null },
    actor: { id: 'a2', name: 'Camille Martin' },
    origin: 'mcp',
    createdAt: '2026-11-06T17:20:00Z',
  }),
  entry('e3', {
    field: 'status',
    old: { kind: 'text', text: 'To do', html: null },
    new: { kind: 'text', text: 'In progress', html: null },
  }),
  entry('e2', {
    field: 'description',
    old: null,
    new: { kind: 'richText', text: null, html: '<p>The new text</p>' },
    createdAt: '2026-11-03T15:05:00Z',
  }),
  entry('e1', { action: 'created', actor: null, origin: 'system', createdAt: '2026-11-02T08:00:00Z' }),
];

const COMMENTS: IssueComment[] = ['c1', 'c2', 'c3'].map((id) => ({
  id,
  author: { id: 'a1', name: 'Lucas Bernard' },
  visibility: 'internal',
  byTransition: false,
  content: { type: 'doc', content: [] },
  contentHtml: '<p>Text</p>',
  deleted: false,
  createdAt: '2026-11-10T09:00:00Z',
  editedAt: null,
  version: 1,
  canEdit: false,
  canDelete: false,
}));

const page = (items: ActivityEntry[], nextCursor: string | null = null): ActivityPage => ({ items, nextCursor });

describe('the Activity tab of the ticket page', () => {
  async function openTicket(activity: ActivityPage | null = page(ENTRIES)) {
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
    TestBed.inject(LocaleService).useTimeZone('Europe/Paris');
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
    const isActivity = (url: string) => url.startsWith('/api/v1/issues/i142/activity');
    const loaded = TestBed.inject(AuthService).load();
    await reply((url) => url === '/api/v1/auth/session', SESSION);
    await loaded;
    const harness = await RouterTestingHarness.create();
    await harness.navigateByUrl('/issues/PLAT-142');
    await reply((url, method) => url.startsWith('/api/v1/issues/PLAT-142') && method === 'GET', ISSUE);
    await reply((url) => url === '/api/v1/nodes/p1/my-permissions', ['view', 'comment']);
    await reply((url) => url === '/api/v1/issues/i142/comments', COMMENTS);
    if (activity) await reply(isActivity, activity);
    const root = () => document.body;
    const tabs = () => root().querySelector('section.tabs')!;
    const panel = () => root().querySelector('app-issue-activity');
    // Icons are ligatures: their text is not read.
    const text = (element: Element | null | undefined) => {
      const copy = element?.cloneNode(true) as Element | undefined;
      copy?.querySelectorAll('mat-icon').forEach((icon) => icon.remove());
      return copy?.textContent?.replace(/\s+/g, ' ').trim();
    };
    const texts = (selector: string, within: Element | null = panel()) => Array.from(within?.querySelectorAll(selector) ?? []).map((e) => text(e));
    const click = async (element: Element | null | undefined) => {
      (element as HTMLElement).click();
      await settle();
    };
    return { http, harness, reply, settle, root, tabs, panel, text, texts, click, isActivity };
  }

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it('shows the activity first, newest first, as sentences with their date, MCP marked', async () => {
    const { tabs, texts, panel, text } = await openTicket();
    expect(texts('[role="tab"]', tabs())).toEqual(['Activity', 'Comments (3)']);
    expect(tabs().querySelector('[role="tab"][aria-selected="true"]')?.textContent?.trim()).toBe('Activity');
    expect(texts('.entry .sentence')).toEqual([
      'Camille Martin changed Application: Directory → Mail via MCP',
      'Lucas Bernard changed the status: To do → In progress',
      'Lucas Bernard changed the description',
      'The system created the ticket',
    ]);
    expect(texts('.entry .avatar')).toEqual(['CM', 'LB', 'LB', 'TS']);
    expect(text(panel()!.querySelector('.entry .date'))).toBe('Friday, November 6, 2026 at 6:20 PM');
    expect(panel()!.querySelector('.entry .origin mat-icon')?.textContent?.trim()).toBe('smart_toy');
    expect(panel()!.querySelector('button.more')).toBeNull();
    expect(await axeViolations(tabs())).toEqual([]);
  });

  it("shows a description's before and after on demand", async () => {
    const { panel, click, text } = await openTicket();
    const toggle = panel()!.querySelector<HTMLButtonElement>('[data-action="updated"] button.toggle')!;
    expect(text(toggle)).toBe('Show before and after');
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    await click(toggle);
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    const change = panel()!.querySelector(`#${toggle.getAttribute('aria-controls')}`)!;
    expect(Array.from(change.querySelectorAll('.side .label')).map((label) => text(label))).toEqual(['Before', 'After']);
    expect(Array.from(change.querySelectorAll('.side .nothing, .side ui-rich-text')).map((side) => text(side))).toEqual([
      'nothing',
      'The new text',
    ]);
    expect(await axeViolations(panel()!)).toEqual([]);
    await click(toggle);
    expect(panel()!.querySelector('.change')).toBeNull();
  });

  it('reads older entries page by page', async () => {
    const { panel, click, reply, isActivity, texts } = await openTicket(page(ENTRIES.slice(0, 2), 'next-1'));
    await click(panel()!.querySelector('button.more'));
    const older = await reply(isActivity, page(ENTRIES.slice(2)));
    expect(older.request.params.get('cursor')).toBe('next-1');
    expect(older.request.params.get('limit')).toBe('20');
    expect(texts('.entry .sentence')).toHaveLength(4);
    expect(panel()!.querySelector('button.more')).toBeNull();
  });

  it('reads the history again when a comment changes, and says when it cannot', async () => {
    const { harness, reply, isActivity, panel, text, texts, tabs, click } = await openTicket();
    const tab = (label: string) => Array.from(tabs().querySelectorAll('[role="tab"]')).find((t) => text(t) === label);
    await click(tab('Comments (3)'));
    const comments = harness.fixture.debugElement.query(By.directive(IssueComments)).componentInstance as IssueComments;
    comments.changed.emit();
    await reply(isActivity, { type: 'about:blank', title: 'Unavailable', status: 503, code: 'unavailable' }, 503);
    await click(tab('Activity'));
    expect(texts('.entry')).toEqual([]);
    expect(panel()!.querySelector('ui-error-state')).not.toBeNull();
    expect(text(panel()!.querySelector('ui-error-state button'))).toBeTruthy();
  });

  it('says when nothing happened yet', async () => {
    const { texts } = await openTicket(page([]));
    expect(texts('.empty')).toEqual(['No activity yet.']);
  });
});
