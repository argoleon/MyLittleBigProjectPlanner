/**
 * Tests of the ticket page in real time (L05-US08), the API mocked at the
 * HTTP level and the stream faked: a change of the ticket shown makes it
 * and its parts read again; another ticket's changes or new notifications
 * do not; while a form is open, someone else's change warns instead, the
 * input kept, until the member reloads; a resync reads everything again.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { Component } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { AuthService, EVENT_SOURCE, EventSourceLike, Issue, IssueTransition, Realtime, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

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

const START: IssueTransition = {
  id: 'tr-start',
  name: 'Start',
  toStatus: { id: 's2', key: 'in_progress', label: 'In progress', category: 'in_progress' },
  fields: [],
};

/** A form left open: the edit dialog, a transition… */
@Component({ template: '<p>A form being filled</p>' })
class OpenForm {}

/** A stream the test drives. */
class FakeSource implements EventSourceLike {
  readyState = 1;
  onerror: ((event: Event) => unknown) | null = null;
  onopen: ((event: Event) => unknown) | null = null;
  private readonly listeners = new Map<string, ((event: MessageEvent) => void)[]>();

  addEventListener(type: string, listener: (event: MessageEvent) => void): void {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }

  close(): void {
    this.readyState = 2;
  }

  emit(type: string, data: object = {}): void {
    for (const listener of this.listeners.get(type) ?? []) listener(new MessageEvent(type, { data: JSON.stringify(data) }));
  }
}

describe('the ticket page in real time', () => {
  async function openTicket(transitions: IssueTransition[] = []) {
    localStorage.clear();
    const source = new FakeSource();
    TestBed.configureTestingModule({
      providers: [
        provideRouter([{ path: 'issues/:key', component: IssuePage }], withComponentInputBinding()),
        provideApi(),
        provideHttpClientTesting(),
        provideI18nTesting(),
        provideMaterialDefaults(),
        { provide: EVENT_SOURCE, useValue: () => source },
      ],
    });
    const http = TestBed.inject(HttpTestingController);
    const settle = async () => {
      for (let i = 0; i < 3; i++) {
        await new Promise((resolve) => setTimeout(resolve));
        TestBed.tick();
      }
    };
    const reply = async (match: (url: string, method: string) => boolean, body: object | null): Promise<TestRequest> => {
      let request: TestRequest | undefined;
      for (let i = 0; i < 50 && !request; i++) {
        [request] = http.match((r) => match(r.urlWithParams, r.method));
        if (!request) await new Promise((resolve) => setTimeout(resolve, 20));
      }
      if (!request) throw new Error('no matching request');
      request.flush(body);
      await settle();
      return request;
    };
    const loaded = TestBed.inject(AuthService).load();
    await reply((url) => url === '/api/v1/auth/session', SESSION);
    await loaded;
    TestBed.inject(Realtime).start();
    const harness = await RouterTestingHarness.create();
    await harness.navigateByUrl('/issues/PLAT-142');
    await reply((url, method) => url.startsWith('/api/v1/issues/PLAT-142') && method === 'GET', ISSUE);
    await reply((url) => url === '/api/v1/nodes/p1/my-permissions', ['view', 'edit', 'comment']);
    await settle();
    /** The requests asked since the last call, by what they read; answered with nothing. */
    const asked = () => {
      const urls = http.match(() => true).map((request) => {
        const url = request.request.urlWithParams;
        request.flush(
          url.includes('/watchers')
            ? { count: 1, watching: true }
            : url.includes('/activity')
              ? { items: [], nextCursor: null }
              : url.startsWith('/api/v1/issues/PLAT-142')
                ? { ...ISSUE }
                : url.includes('/transitions')
                  ? transitions
                  : [],
        );
        return url.replace(/\?.*$/, '');
      });
      return [...new Set(urls)].sort();
    };
    /** What is asked over two rounds: some parts read once the ticket is read. */
    const askedAll = async () => {
      const first = asked();
      await settle();
      await settle();
      return [...new Set([...first, ...asked()])].sort();
    };
    asked();
    const root = () => document.body;
    return { source, settle, asked, askedAll, root };
  }

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it("reads the ticket and its parts again when someone changes it, not for another ticket's changes", async () => {
    const { source, settle, asked, askedAll } = await openTicket();
    source.emit('issue', { topic: 'issue', kind: 'issue.transitioned', issueId: 'i7', issueKey: 'PLAT-7', actorId: 'a2' });
    source.emit('notifications');
    await settle();
    expect(asked()).toEqual([]);
    source.emit('issue', { topic: 'issue', kind: 'issue.transitioned', issueId: 'i142', issueKey: 'PLAT-142', actorId: 'a2' });
    await settle();
    await settle();
    expect(await askedAll()).toEqual([
      '/api/v1/issues/PLAT-142',
      '/api/v1/issues/i142/activity',
      '/api/v1/issues/i142/attachments',
      '/api/v1/issues/i142/comments',
      '/api/v1/issues/i142/transitions',
      '/api/v1/issues/i142/watchers',
    ]);
  });

  it('warns instead while a form is open, keeps the input, and reads again on demand', async () => {
    const { source, settle, asked, root } = await openTicket();
    TestBed.inject(MatDialog).open(OpenForm);
    await settle();
    asked();
    source.emit('issue', { topic: 'issue', kind: 'issue.updated', issueId: 'i142', issueKey: 'PLAT-142', actorId: 'a2' });
    await settle();
    expect(asked()).toEqual([]);
    const banner = root().querySelector('.remote[role="status"]');
    expect(banner?.textContent).toContain('Someone else changed this ticket while your form was open');
    expect(TestBed.inject(MatDialog).openDialogs.length, 'the form stays').toBe(1);
    expect(await axeViolations(root().querySelector('.remote')!)).toEqual([]);
    TestBed.inject(MatDialog).closeAll();
    await settle();
    asked();
    (root().querySelector('.remote button.reload') as HTMLButtonElement).click();
    await settle();
    await settle();
    expect(asked()).toContain('/api/v1/issues/PLAT-142');
    expect(root().querySelector('.remote')).toBeNull();
  });

  it('keeps the status menu open while the echo of its own change reads the ticket again', async () => {
    const { source, settle, asked, root } = await openTicket([START]);
    await settle();
    asked();
    await settle();
    const status = () => root().querySelector('app-issue-status')!;
    (status().querySelector('button.status-menu') as HTMLButtonElement).click();
    await settle();
    const item = root().querySelector('button.transition');
    expect(item?.textContent).toContain('Start');
    source.emit('issue', { topic: 'issue', kind: 'issue.transitioned', issueId: 'i142', issueKey: 'PLAT-142', actorId: 'a1' });
    await settle();
    // The ticket is read again, then its transitions: while they load, the menu stays.
    expect(asked()).toContain('/api/v1/issues/PLAT-142');
    await settle();
    expect(item?.isConnected, 'the open menu is not rebuilt').toBe(true);
    expect(status().querySelector('ui-status-chip')).toBeNull();
    expect(asked()).toContain('/api/v1/issues/i142/transitions');
    await settle();
    expect(item?.isConnected).toBe(true);
  });

  it('reads everything again on a resync, and after its own changes even with a form open', async () => {
    const { source, settle, asked, root } = await openTicket();
    source.emit('resync');
    await settle();
    await settle();
    expect(asked()).toContain('/api/v1/issues/i142/comments');
    TestBed.inject(MatDialog).open(OpenForm);
    await settle();
    asked();
    source.emit('issue', { topic: 'issue', kind: 'issue.updated', issueId: 'i142', issueKey: 'PLAT-142', actorId: 'a1' });
    await settle();
    await settle();
    expect(asked()).toContain('/api/v1/issues/PLAT-142');
    expect(root().querySelector('.remote')).toBeNull();
  });
});
