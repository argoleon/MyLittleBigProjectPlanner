/**
 * Tests of comments on the ticket page (L05-US04), with the API mocked at
 * the HTTP level and the real translations: the list (author, date,
 * visibility, rendered content, placeholder), a comment written public, a
 * refusal said, an edit with the version read and its conflict, a deletion
 * once confirmed, the people and tickets the editor offers, readers who
 * only read, and accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { By } from '@angular/platform-browser';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { AuthService, Issue, IssueComment, provideApi, provideMaterialDefaults } from 'ui';
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
  status: { id: 's1', key: 'todo', label: 'To do', category: 'todo' },
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
  createdAt: '2026-11-06T09:12:00Z',
  updatedAt: '2026-11-06T18:20:00Z',
  version: 4,
};

const comment = (id: string, rest: Partial<IssueComment> = {}): IssueComment => ({
  id,
  author: { id: 'a1', name: 'Lucas Bernard' },
  visibility: 'internal',
  byTransition: false,
  content: { type: 'doc', content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Text' }] }] },
  contentHtml: '<p>Text</p>',
  deleted: false,
  createdAt: '2026-11-10T09:00:00Z',
  editedAt: null,
  version: 1,
  canEdit: false,
  canDelete: false,
  ...rest,
});

const COMMENTS: IssueComment[] = [
  comment('c1', {
    author: { id: 'a2', name: 'Camille Martin' },
    contentHtml: '<p><span class="mention" data-id="a1">@Lucas Bernard</span> see <a class="issue-ref" href="/issues/PLAT-120">PLAT-120</a></p>',
  }),
  comment('c2', { visibility: 'public', editedAt: '2026-11-11T09:00:00Z', canEdit: true, canDelete: true, version: 2 }),
  comment('c3', { deleted: true, content: null, contentHtml: null }),
  comment('c4', { byTransition: true, contentHtml: '<p>Validated</p>' }),
];

const DOC = { type: 'doc', content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Deployed' }] }] };

describe('comments on the ticket page', () => {
  async function openTicket(permissions: string[] = ['view', 'comment']) {
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
    const isList = (url: string, method: string) => url === '/api/v1/issues/i142/comments' && method === 'GET';
    const loaded = TestBed.inject(AuthService).load();
    await reply((url) => url === '/api/v1/auth/session', SESSION);
    await loaded;
    const harness = await RouterTestingHarness.create();
    await harness.navigateByUrl('/issues/PLAT-142');
    await reply((url, method) => url.startsWith('/api/v1/issues/PLAT-142') && method === 'GET', ISSUE);
    await reply((url) => url === '/api/v1/nodes/p1/my-permissions', permissions);
    await reply(isList, COMMENTS);
    const root = () => document.body;
    // The comments are in their tab, next to the activity (L05-US06).
    const commentsTab = Array.from(root().querySelectorAll<HTMLElement>('[role="tab"]')).find((tab) => tab.textContent?.includes('Comments'));
    commentsTab!.click();
    await settle();
    const section = () => root().querySelector('app-issue-comments')!;
    const component = () => harness.fixture.debugElement.query(By.directive(IssueComments)).componentInstance as IssueComments;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const texts = (selector: string, within: Element = section()) => Array.from(within.querySelectorAll(selector)).map((e) => text(e));
    const click = async (element: Element | null | undefined) => {
      (element as HTMLElement).click();
      await settle();
    };
    return { http, reply, settle, root, section, component, text, texts, click, isList };
  }

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it('lists the comments with their author, visibility and content, a placeholder once deleted', async () => {
    const { section, texts, text, root } = await openTicket();
    expect(text(root().querySelector('[role="tab"][aria-selected="true"]'))).toBe('Comments (3)');
    expect(texts('.comment .author')).toEqual(['Camille Martin', 'Lucas Bernard', 'Lucas Bernard', 'Lucas Bernard']);
    expect(texts('.comment .visibility')).toEqual(['Internal', 'Public', 'Internal']);
    expect(text(section().querySelector('.comment .mention'))).toBe('@Lucas Bernard');
    expect(section().querySelector('.comment a.issue-ref')?.getAttribute('href')).toBe('/issues/PLAT-120');
    expect(texts('.comment .note')).toEqual(['edited', 'via the workflow']);
    expect(texts('.comment .deleted')).toEqual(['Comment deleted.']);
    // Only the comment the member may change offers its buttons.
    expect(Array.from(section().querySelectorAll('button.edit, button.remove')).map((b) => b.getAttribute('aria-label'))).toEqual([
      'Edit the comment of Lucas Bernard',
      'Delete the comment of Lucas Bernard',
    ]);
    expect(await axeViolations(section())).toEqual([]);
  });

  it('writes a public comment, then lists it; a refusal is said', async () => {
    const { section, component, click, reply, isList, text } = await openTicket();
    component()['content'].setValue(DOC);
    await click(Array.from(section().querySelectorAll('.new mat-button-toggle button')).find((b) => text(b) === 'Public (portal)'));
    await click(section().querySelector('button.write'));
    const refused = await reply(
      (url, method) => url === '/api/v1/issues/i142/comments' && method === 'POST',
      { type: 'about:blank', title: 'Invalid', status: 400, code: 'validation_failed', errors: [{ field: 'content', code: 'unknown_member' }] },
      400,
    );
    expect(refused.request.body).toEqual({ content: DOC, visibility: 'public' });
    expect(text(section().querySelector('[role="alert"]'))).toBe('Not a member of the organization.');
    await click(section().querySelector('button.write'));
    await reply((url, method) => method === 'POST' && url.endsWith('/comments'), comment('c5'), 201);
    await reply(isList, [...COMMENTS, comment('c5')]);
    expect(section().querySelectorAll('.comment')).toHaveLength(5);
    expect(component()['content'].value).toBeNull();
  });

  it('edits a comment with the version read, and says when it changed meanwhile', async () => {
    const { section, component, click, reply, isList, text } = await openTicket();
    await click(section().querySelector('button.edit'));
    expect(section().querySelector('.editor ui-rich-text-editor')).not.toBeNull();
    component()['editContent'].setValue(DOC);
    await click(section().querySelector('button.save'));
    await reply(
      (url, method) => url === '/api/v1/issues/i142/comments/c2' && method === 'PUT',
      { type: 'about:blank', title: 'Precondition failed', status: 412, code: 'precondition_failed' },
      412,
    );
    expect(text(section().querySelector('.editor [role="alert"]'))).toContain('This comment changed since you opened it');
    await click(section().querySelector('button.save'));
    const sent = await reply((url, method) => method === 'PUT' && url.endsWith('/comments/c2'), comment('c2', { version: 3 }));
    expect(sent.request.headers.get('If-Match')).toBe('"2"');
    expect(sent.request.body).toEqual({ content: DOC, visibility: 'public' });
    await reply(isList, COMMENTS);
    expect(section().querySelector('.editor')).toBeNull();
  });

  it('deletes a comment once confirmed', async () => {
    const { section, root, click, reply, isList, text } = await openTicket();
    await click(section().querySelector('button.remove'));
    const confirm = root().querySelector('ui-confirm-dialog')!;
    expect(text(confirm.querySelector('h2'))).toBe('Delete the comment?');
    await click(Array.from(confirm.querySelectorAll('button')).find((b) => text(b) === 'Delete'));
    await reply((url, method) => url === '/api/v1/issues/i142/comments/c2' && method === 'DELETE', null, 204);
    await reply(isList, COMMENTS);
  });

  it('offers the people of the project after @ and tickets after #', async () => {
    const { component, reply } = await openTicket();
    const people = component()['people']('cam');
    const searched = await reply((url) => url.startsWith('/api/v1/projects/PLAT/people'), [{ id: 'a2', name: 'Camille Martin' }]);
    expect(searched.request.params.get('search')).toBe('cam');
    expect(await people).toEqual([{ id: 'a2', label: 'Camille Martin' }]);
    const tickets = component()['tickets']('dir');
    await reply((url) => url.startsWith('/api/v1/issues?'), [
      { id: 'i120', key: 'PLAT-120', summary: 'Directory scoping', project: ISSUE.project, type: ISSUE.type, status: ISSUE.status },
    ]);
    expect(await tickets).toEqual([{ id: 'PLAT-120', label: 'PLAT-120', detail: 'Directory scoping' }]);
    expect(await component()['tickets']('  ')).toEqual([]);
  });

  it('lets readers read, not write', async () => {
    const { section } = await openTicket(['view']);
    expect(section().querySelector('.new')).toBeNull();
    expect(section().querySelectorAll('.comment')).toHaveLength(4);
  });
});
