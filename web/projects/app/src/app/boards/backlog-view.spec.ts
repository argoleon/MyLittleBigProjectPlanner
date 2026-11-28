/**
 * Tests of the Scrum backlog (L09-US01) through the board page, the API
 * mocked at the HTTP level, the stream faked and the real translations: the
 * open sprints with their totals, another board's sprint read-only, the
 * ranked backlog; a ticket dropped in a sprint or moved with the keyboard
 * (to a sprint, up, down, top, bottom), a refusal shown and the backlog read
 * again; sprints created, changed and deleted; a member who only reads;
 * others' changes read as they happen; failures and accessibility.
 */
import { CdkDropList } from '@angular/cdk/drag-drop';
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { By } from '@angular/platform-browser';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import {
  Backlog,
  BacklogItem,
  BacklogSprint,
  Board,
  BoardCard,
  ConfirmDialog,
  EVENT_SOURCE,
  EventSourceLike,
  Realtime,
  addDays,
  provideApi,
  provideMaterialDefaults,
  toLocalDate,
} from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { REALTIME_DELAY_MS } from './backlog-view';
import { BoardPage } from './board-page';
import { CloseSprintDialog } from './close-sprint-dialog';
import { SprintDialog } from './sprint-dialog';
import { SprintScopeDialog } from './sprint-scope-dialog';

const BOARD: Board = {
  id: 'b1',
  node: { id: 'p1', key: 'PLAT', name: 'Identity platform', nodeType: 'project', archived: false },
  name: 'Platform Scrum',
  kind: 'scrum',
  filter: 'project = PLAT ORDER BY key',
  estimation: 'points',
  swimlanes: 'none',
  quickFilters: [],
  columns: [],
  canManage: true,
  canPlan: true,
  version: 1,
};

const TODO = { id: 's1', key: 'todo', label: 'To do', category: 'todo' as const };
const DONE = { id: 's4', key: 'done', label: 'Done', category: 'done' as const };

function item(n: number, estimate: number | null, overrides: Partial<BoardCard> = {}, subtasks = 0): BacklogItem {
  return {
    card: {
      id: `i${n}`,
      key: `PLAT-${n}`,
      summary: `Ticket ${n}`,
      projectId: 'p1',
      type: { id: 't1', key: 'story', label: 'Story', labelTranslations: { en: 'Story' }, icon: 'bookmark', color: 'story', isSubtask: false, parentIds: [] },
      status: TODO,
      priority: 'medium',
      assignee: null,
      estimate,
      epic: null,
      blockedBy: [],
      version: 1,
      ...overrides,
    },
    subtasks,
  };
}

function sprint(id: string, name: string, items: BacklogItem[], overrides: Partial<BacklogSprint> = {}, sprintFields: object = {}): BacklogSprint {
  const estimate = items.reduce((sum, i) => sum + (i.card.estimate ?? 0), 0);
  return {
    sprint: { id, boardId: 'b1', boardName: 'Platform Scrum', name, goal: null, startDate: null, endDate: null, state: 'future', version: 1, ...sprintFields },
    own: true,
    items,
    totals: { count: items.length, estimate, done: 0, unestimated: items.filter((i) => i.card.estimate === null).length },
    ...overrides,
  };
}

function backlog(overrides: Partial<Backlog> = {}): Backlog {
  const rest = [item(3, 5, {}, 2), item(4, null), item(5, 1)];
  return {
    estimation: 'points',
    canPlan: true,
    truncated: false,
    sprints: [
      sprint('sp1', 'PLAT Sprint 1', [item(1, 8, { epic: { id: 'e1', key: 'PLAT-100', summary: 'Unified authentication' } }), item(2, 3, { status: DONE })], {}, {
        goal: 'OIDC in acceptance',
        startDate: '2026-11-16',
        endDate: '2026-11-27',
      }),
      sprint('sp2', 'Sprint 25', []),
      sprint('x1', 'Team A Sprint 3', [item(9, 2)], { own: false }, { boardId: 'b2', boardName: 'Team A' }),
    ],
    backlog: { items: rest, totals: { count: 3, estimate: 6, done: 0, unestimated: 1 } },
    ...overrides,
  };
}

/** The cards of a Scrum board without an active sprint. */
const NO_SPRINT = { columns: [], swimlanes: 'none', lanes: [], total: 0, unmapped: 0, truncated: false, sprint: null };

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

describe('the Scrum backlog', () => {
  async function open(first: Backlog | null = backlog(), board: Board = BOARD, cards: object = NO_SPRINT) {
    const source = new FakeSource();
    TestBed.configureTestingModule({
      providers: [
        provideRouter([{ path: 'boards/:id', component: BoardPage }], withComponentInputBinding()),
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
        await new Promise((resolve) => setTimeout(resolve, 20));
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
      request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error', headers: status >= 400 ? { 'content-type': 'application/problem+json' } : {} });
      await settle();
      return request;
    };
    TestBed.inject(Realtime).start();
    const harness = await RouterTestingHarness.create();
    await harness.navigateByUrl('/boards/b1');
    await reply((url, method) => url === '/api/v1/boards/b1' && method === 'GET', board);
    // Without an active sprint, the board opens on its backlog.
    await reply((url, method) => url === '/api/v1/boards/b1/cards?swimlanes=none' && method === 'GET', cards);
    const backlogAsked = (url: string, method: string) => url === '/api/v1/boards/b1/backlog' && method === 'GET';
    if ('sprint' in cards && cards.sprint) {
      // On the active sprint: the backlog is one click away.
      (harness.routeNativeElement as HTMLElement).querySelectorAll<HTMLElement>('.views button')[0].click();
      await settle();
    }
    if (first) await reply(backlogAsked, first);
    else await reply(backlogAsked, { code: 'internal', status: 500, title: 'Internal' }, 500);
    const page = () => harness.routeNativeElement as HTMLElement;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const texts = (selector: string, within: ParentNode = page()) => Array.from(within.querySelectorAll(selector)).map((e) => text(e));
    const click = async (element: Element | null | undefined) => {
      if (!element) throw new Error('nothing to click');
      (element as HTMLElement).click();
      await settle();
    };
    /** The keys of each list, sprints first. */
    const lists = () => Array.from(page().querySelectorAll('.list')).map((list) => texts('app-backlog-row .key', list));
    const dropLists = () => harness.fixture.debugElement.queryAll(By.directive(CdkDropList)).map((d) => d.injector.get(CdkDropList));
    const row = (key: string) => Array.from(page().querySelectorAll('app-backlog-row')).find((r) => text(r.querySelector('.key')) === key);
    const isMove = (url: string, method: string) => url === '/api/v1/boards/b1/backlog/moves' && method === 'POST';
    return { http, harness, page, reply, settle, text, texts, click, lists, dropLists, row, backlogAsked, isMove, source };
  }

  afterEach(async () => {
    TestBed.inject(MatDialog).closeAll();
    await new Promise((resolve) => setTimeout(resolve, 200));
    TestBed.inject(HttpTestingController).verify({ ignoreCancelled: true });
  });

  it('shows the open sprints with their totals, another board’s sprint read-only, and the ranked backlog', async () => {
    const { page, http, text, texts, lists, row } = await open();
    // No Kanban columns for a Scrum board.
    expect(http.match((r) => r.url.includes('/cards')).length).toBe(0);
    expect(page().querySelector('.board')).toBeNull();
    expect(text(page().querySelector('h1'))).toBe('Platform Scrum');
    expect(text(page().querySelector('.estimation'))).toBe('Estimated in points');
    expect(texts('.list .name')).toEqual(['PLAT Sprint 1', 'Sprint 25', 'Team A Sprint 3', 'Backlog']);
    expect(lists()).toEqual([['PLAT-1', 'PLAT-2'], [], ['PLAT-9'], ['PLAT-3', 'PLAT-4', 'PLAT-5']]);
    const first = page().querySelectorAll('.list')[0];
    expect(text(first.querySelector('.state'))).toBe('Planned');
    expect(text(first.querySelector('.meta'))).toContain('Goal: OIDC in acceptance');
    expect(text(first.querySelector('.meta'))).toMatch(/16.* → .*27/);
    expect(text(first.querySelector('.totals'))).toBe('2 tickets · 11 points');
    expect(text(page().querySelectorAll('.list')[1].querySelector('.empty'))).toBe('Drag tickets here from the backlog to plan them.');
    const foreign = page().querySelectorAll('.list')[2];
    expect(foreign.classList).toContain('foreign');
    expect(text(foreign.querySelector('.other'))).toBe('Planned by the board Team A, read-only here');
    expect(foreign.querySelector('.sprint-menu')).toBeNull();
    expect(row('PLAT-9')!.querySelector('.handle')).toBeNull();
    expect(row('PLAT-9')!.querySelector('.row-menu')).toBeNull();
    expect(text(page().querySelectorAll('.list')[3].querySelector('.meta'))).toBe(
      '3 tickets · 6 points · 1 not estimated · drag a ticket to a sprint to plan it',
    );
    // A row: link, sub-tasks, epic, estimate.
    expect(row('PLAT-1')!.querySelector('a.key')?.getAttribute('href')).toBe('/issues/PLAT-1');
    expect(text(row('PLAT-1')!.querySelector('.epic'))).toBe('Unified authentication');
    expect(text(row('PLAT-3')!.querySelector('.subtasks'))).toBe('2 sub-tasks');
    expect(row('PLAT-3')!.querySelector('.estimate')?.getAttribute('aria-label')).toBe('5 points');
    expect(row('PLAT-4')!.querySelector('.estimate')?.getAttribute('aria-label')).toBe('Not estimated');
    expect(row('PLAT-1')!.querySelector('.handle')).not.toBeNull();
    expect(page().querySelector('.create-sprint')).not.toBeNull();
  });

  it('plans a dropped ticket at once, then for real, and puts it back when refused', async () => {
    const { reply, settle, lists, dropLists, isMove, backlogAsked, page, text } = await open();
    const [sprint1, , , rest] = dropLists();
    const dragged = item(4, null);
    sprint1.dropped.emit({ previousContainer: rest, container: sprint1, item: { data: dragged }, previousIndex: 1, currentIndex: 1 } as never);
    await settle();
    // In the sprint at once, with its totals.
    expect(lists()[0]).toEqual(['PLAT-1', 'PLAT-4', 'PLAT-2']);
    expect(lists()[3]).toEqual(['PLAT-3', 'PLAT-5']);
    expect(text(page().querySelectorAll('.list')[0].querySelector('.totals'))).toBe('3 tickets · 11 points · 1 not estimated');
    const moved = await reply(isMove, null, 204);
    expect(moved.request.body).toEqual({ issueId: 'i4', sprintId: 'sp1', afterId: 'i1', beforeId: null });
    await reply(backlogAsked, backlog());

    // Reranked within the backlog: on top.
    rest.dropped.emit({ previousContainer: rest, container: rest, item: { data: item(5, 1) }, previousIndex: 2, currentIndex: 0 } as never);
    await settle();
    expect(lists()[3]).toEqual(['PLAT-5', 'PLAT-3', 'PLAT-4']);
    const ranked = await reply(isMove, null, 204);
    expect(ranked.request.body).toEqual({ issueId: 'i5', sprintId: null, afterId: null, beforeId: 'i3' });
    await reply(backlogAsked, backlog());

    // Refused: the reason shows, the backlog is read again.
    sprint1.dropped.emit({ previousContainer: rest, container: sprint1, item: { data: item(3, 5) }, previousIndex: 0, currentIndex: 0 } as never);
    await reply(isMove, { code: 'sprint.foreign', status: 409, title: 'Foreign' }, 409);
    expect(text(page().querySelector('[role="alert"] span'))).toBe('PLAT-3 is planned by another board: it stays there.');
    await reply(backlogAsked, backlog());
    expect(lists()[3]).toEqual(['PLAT-3', 'PLAT-4', 'PLAT-5']);
    // Dropped where it was: nothing asked.
    rest.dropped.emit({ previousContainer: rest, container: rest, item: { data: item(3, 5) }, previousIndex: 0, currentIndex: 0 } as never);
    await settle();
  });

  it('plans and ranks with the keyboard through each ticket’s menu', async () => {
    const { click, reply, isMove, backlogAsked, row } = await open();
    const menu = row('PLAT-4')!.querySelector('.row-menu') as HTMLElement;
    expect(menu.getAttribute('aria-label')).toBe('Plan PLAT-4');
    await click(menu);
    const items = () => Array.from(document.querySelectorAll('.mat-mdc-menu-panel button')).map((b) => b.textContent?.trim());
    expect(items()).toEqual(['Move to PLAT Sprint 1', 'Move to Sprint 25', 'Move to the top', 'Move up', 'Move down', 'Move to the bottom']);
    await click(document.querySelectorAll('.send-to')[1]);
    let moved = await reply(isMove, null, 204);
    expect(moved.request.body).toEqual({ issueId: 'i4', sprintId: 'sp2', afterId: null, beforeId: null });
    await reply(backlogAsked, backlog());

    for (const [action, body] of [
      ['.move-top', { afterId: null, beforeId: 'i3' }],
      ['.move-up', { afterId: null, beforeId: 'i3' }],
      ['.move-down', { afterId: 'i5', beforeId: null }],
      ['.move-bottom', { afterId: 'i5', beforeId: null }],
    ] as const) {
      await click(row('PLAT-4')!.querySelector('.row-menu'));
      await click(document.querySelector(action));
      moved = await reply(isMove, null, 204);
      expect(moved.request.body).toEqual({ issueId: 'i4', sprintId: null, ...body });
      await reply(backlogAsked, backlog());
    }

    // From a sprint: back to the backlog; the first of a list cannot go up.
    await click(row('PLAT-1')!.querySelector('.row-menu'));
    expect(items()).toEqual(['Move to Sprint 25', 'Move to the backlog', 'Move down', 'Move to the bottom']);
    await click(document.querySelectorAll('.send-to')[1]);
    moved = await reply(isMove, null, 204);
    expect(moved.request.body).toEqual({ issueId: 'i1', sprintId: null, afterId: 'i5', beforeId: null });
    await reply(backlogAsked, backlog());
  });

  it('creates, changes and deletes sprints', async () => {
    const { page, click, reply, backlogAsked, settle } = await open();
    const dialogs = () => TestBed.inject(MatDialog).openDialogs;
    await click(page().querySelector('.create-sprint'));
    expect(dialogs()[0].componentInstance).toBeInstanceOf(SprintDialog);
    const pane = () => document.querySelector('app-sprint-dialog') as HTMLElement;
    expect(pane().querySelector('h2')?.textContent?.trim()).toBe('New sprint');
    // One date only is refused before asking.
    const start = pane().querySelector('input[matStartDate]') as HTMLInputElement;
    start.value = '11/30/2026';
    start.dispatchEvent(new Event('input'));
    await settle();
    await click(pane().querySelector('button.save'));
    expect(pane().querySelector('.dates-error')?.textContent?.trim()).toBe('Give both dates, or none.');
    start.value = '';
    start.dispatchEvent(new Event('input'));
    const goal = pane().querySelector('textarea') as HTMLTextAreaElement;
    goal.value = ' Audit of sign-ins ';
    goal.dispatchEvent(new Event('input'));
    await settle();
    await click(pane().querySelector('button.save'));
    const created = await reply((url, method) => url === '/api/v1/boards/b1/sprints' && method === 'POST', {
      ...backlog().sprints[1].sprint,
      id: 'sp3',
      name: 'PLAT Sprint 3',
    });
    expect(created.request.body).toEqual({ name: null, goal: 'Audit of sign-ins', startDate: null, endDate: null });
    await reply(backlogAsked, backlog());
    expect(dialogs().length).toBe(0);

    // Renamed: a field error lands on its field.
    await click(page().querySelectorAll('.sprint-menu')[1]);
    await click(document.querySelector('.edit-sprint'));
    const name = pane().querySelector('input[formControlName="name"]') as HTMLInputElement;
    expect(name.value).toBe('Sprint 25');
    name.value = 'Sprint 26';
    name.dispatchEvent(new Event('input'));
    await click(pane().querySelector('button.save'));
    await reply(
      (url, method) => url === '/api/v1/sprints/sp2' && method === 'PUT',
      { code: 'validation_failed', status: 400, title: 'Invalid', errors: [{ field: 'name', code: 'too_long' }] },
      400,
    );
    expect(pane().querySelector('mat-error')?.textContent?.trim()).toBe('The text is too long.');
    name.value = 'Sprint 26';
    name.dispatchEvent(new Event('input'));
    await click(pane().querySelector('button.save'));
    const changed = await reply((url, method) => url === '/api/v1/sprints/sp2' && method === 'PUT', { ...backlog().sprints[1].sprint, name: 'Sprint 26', version: 2 });
    expect(changed.request.headers.get('If-Match')).toBe('"1"');
    expect(changed.request.body).toEqual({ name: 'Sprint 26', goal: null, startDate: null, endDate: null });
    await reply(backlogAsked, backlog());

    // Deleted once confirmed: its tickets go back to the backlog.
    await click(page().querySelectorAll('.sprint-menu')[0]);
    await click(document.querySelector('.delete-sprint'));
    const confirm = dialogs().find((d) => d.componentInstance instanceof ConfirmDialog);
    expect(document.querySelector('mat-dialog-content')?.textContent).toContain('Its 2 tickets go back to the backlog');
    confirm!.close(true);
    await settle();
    const deleted = await reply((url, method) => url === '/api/v1/sprints/sp1' && method === 'DELETE', null, 204);
    expect(deleted.request.headers.get('If-Match')).toBe('"1"');
    await reply(backlogAsked, backlog());
  });

  it('shows the backlog read-only to a member who does not plan', async () => {
    const { page, row, dropLists } = await open(backlog({ canPlan: false }), { ...BOARD, canPlan: false });
    expect(page().querySelector('.create-sprint')).toBeNull();
    expect(page().querySelector('.sprint-menu')).toBeNull();
    expect(page().querySelector('.handle')).toBeNull();
    expect(page().querySelector('.row-menu')).toBeNull();
    expect(row('PLAT-3')).toBeDefined();
    expect(dropLists().every((list) => list.disabled)).toBe(true);
  });

  it('reads the backlog again when others change tickets', async () => {
    const { source, settle, http, reply, backlogAsked } = await open();
    source.emit('issue', { topic: 'issue', kind: 'issue.planned', issueId: 'i3', issueKey: 'PLAT-3', actorId: 'a9' });
    source.emit('issue', { topic: 'issue', kind: 'issue.ranked', issueId: 'i4', issueKey: 'PLAT-4', actorId: 'a9' });
    await settle();
    expect(http.match((r) => r.url.includes('/backlog')).length).toBe(0);
    await new Promise((resolve) => setTimeout(resolve, REALTIME_DELAY_MS + 50));
    await settle();
    await reply(backlogAsked, backlog());
    expect(http.match((r) => r.url.includes('/backlog')).length).toBe(0);
  });

  it('tells when the backlog cannot be read', async () => {
    const { page } = await open(null);
    expect(page().querySelector('app-backlog ui-error-state')).not.toBeNull();
  });

  it('passes the accessibility checks', async () => {
    const { page } = await open();
    expect(await axeViolations(page())).toEqual([]);
  });
});

describe('the sprint cycle', () => {
  /** A backlog whose first sprint is active: one ticket done, one not. */
  function running(): Backlog {
    const base = backlog();
    const active = sprint('sp1', 'PLAT Sprint 1', [item(1, 8, { status: DONE }), item(2, 3)], {}, {
      state: 'active',
      goal: 'OIDC in acceptance',
      startDate: '2026-11-16',
      endDate: '2026-11-27',
      committedCount: 2,
      committedEstimate: 11,
    });
    return { ...base, sprints: [{ ...active, totals: { count: 2, estimate: 11, done: 8, unestimated: 0 } }, base.sprints[1]] };
  }

  const activeCards = (overrides: object = {}) => ({
    columns: [
      { id: 'c-todo', name: 'To do', statuses: ['todo'], wipLimit: null, count: 1, wip: 'unlimited' },
      { id: 'c-done', name: 'Done', statuses: ['done'], wipLimit: null, count: 1, wip: 'unlimited' },
    ],
    swimlanes: 'none',
    lanes: [{ id: null, key: null, label: null, cells: [[item(2, 3).card], [item(1, 8, { status: DONE }).card]] }],
    total: 2,
    unmapped: 0,
    truncated: false,
    sprint: running().sprints[0].sprint,
    ...overrides,
  });

  async function open(first: Backlog | null, cards: object = { columns: [], swimlanes: 'none', lanes: [], total: 0, unmapped: 0, truncated: false, sprint: null }) {
    const source = new FakeSource();
    TestBed.configureTestingModule({
      providers: [
        provideRouter([{ path: 'boards/:id', component: BoardPage }], withComponentInputBinding()),
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
        await new Promise((resolve) => setTimeout(resolve, 20));
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
      request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error', headers: status >= 400 ? { 'content-type': 'application/problem+json' } : {} });
      await settle();
      return request;
    };
    TestBed.inject(Realtime).start();
    const harness = await RouterTestingHarness.create();
    await harness.navigateByUrl('/boards/b1');
    await reply((url, method) => url === '/api/v1/boards/b1' && method === 'GET', BOARD);
    const cardsAsked = (url: string, method: string) => url === '/api/v1/boards/b1/cards?swimlanes=none' && method === 'GET';
    await reply(cardsAsked, cards);
    const backlogAsked = (url: string, method: string) => url === '/api/v1/boards/b1/backlog' && method === 'GET';
    if (first) await reply(backlogAsked, first);
    const page = () => harness.routeNativeElement as HTMLElement;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const click = async (element: Element | null | undefined) => {
      if (!element) throw new Error('nothing to click');
      (element as HTMLElement).click();
      await settle();
    };
    const dialogs = () => TestBed.inject(MatDialog).openDialogs;
    return { http, harness, page, reply, settle, text, click, dialogs, cardsAsked, backlogAsked };
  }

  afterEach(async () => {
    TestBed.inject(MatDialog).closeAll();
    await new Promise((resolve) => setTimeout(resolve, 200));
    TestBed.inject(HttpTestingController).verify({ ignoreCancelled: true });
  });

  it('starts a future sprint with its dates, then shows its board', async () => {
    const { page, click, reply, text, dialogs, cardsAsked, settle } = await open(backlog());
    const starts = page().querySelectorAll('.start-sprint');
    expect(starts.length).toBe(2);
    await click(starts[0]);
    expect(dialogs()[0].componentInstance).toBeInstanceOf(SprintDialog);
    const pane = document.querySelector('app-sprint-dialog') as HTMLElement;
    expect(text(pane.querySelector('h2'))).toBe('Start the sprint');
    // Its own dates, kept.
    expect((pane.querySelector('input[matStartDate]') as HTMLInputElement).value).not.toBe('');
    await click(pane.querySelector('button.save'));
    const started = await reply((url, method) => url === '/api/v1/sprints/sp1/start' && method === 'POST', {
      ...running().sprints[0].sprint,
    });
    expect(started.request.headers.get('If-Match')).toBe('"1"');
    expect(started.request.body).toEqual({ name: 'PLAT Sprint 1', goal: 'OIDC in acceptance', startDate: '2026-11-16', endDate: '2026-11-27' });
    await settle();
    // The board switches to the active sprint.
    await reply(cardsAsked, activeCards());
    expect(text(page().querySelector('.sprint-name'))).toBe('PLAT Sprint 1');
    expect(text(page().querySelector('.sprint-goal'))).toBe('Goal: OIDC in acceptance');
    expect(text(page().querySelector('.sprint-left'))).toMatch(/days ago|days left|Last day/);
    expect(page().querySelector('app-backlog')).toBeNull();
    expect(page().querySelectorAll('app-board-card').length).toBe(2);
  });

  it('starts a sprint without dates over two weeks from today', async () => {
    const undated = backlog();
    const { page, click, reply } = await open({ ...undated, sprints: [undated.sprints[1]] });
    await click(page().querySelector('.start-sprint'));
    const pane = document.querySelector('app-sprint-dialog') as HTMLElement;
    await click(pane.querySelector('button.save'));
    const today = toLocalDate(new Date());
    const started = await reply((url) => url === '/api/v1/sprints/sp2/start', { ...undated.sprints[1].sprint, state: 'active' });
    expect(started.request.body).toEqual({ name: 'Sprint 25', goal: null, startDate: today, endDate: addDays(today, 13) });
    await reply((url) => url.startsWith('/api/v1/boards/b1/cards'), activeCards());
  });

  it('asks before changing the scope of the active sprint, then closes it', async () => {
    const { page, harness, click, reply, settle, http, text, dialogs, backlogAsked } = await open(null, activeCards());
    // On the active sprint, then to the backlog.
    expect(text(page().querySelector('.sprint-name'))).toBe('PLAT Sprint 1');
    await click(page().querySelectorAll('.views button')[0]);
    await reply(backlogAsked, running());
    expect(page().querySelector('.start-sprint')).toBeNull();
    expect(text(page().querySelector('.progress-done .totals'))).toBe('8 / 11 points done');

    const lists = harness.fixture.debugElement.queryAll(By.directive(CdkDropList)).map((d) => d.injector.get(CdkDropList));
    const [active, , rest] = lists;
    active.dropped.emit({ previousContainer: rest, container: active, item: { data: item(4, null) }, previousIndex: 1, currentIndex: 2 } as never);
    await settle();
    const confirm = () => dialogs().find((d) => d.componentInstance instanceof ConfirmDialog);
    expect(document.querySelector('mat-dialog-content')?.textContent).toContain('PLAT-4 will join PLAT Sprint 1, which has started');
    confirm()!.close(false);
    await settle();
    expect(http.match((r) => r.url.includes('/backlog/moves')).length).toBe(0);
    active.dropped.emit({ previousContainer: rest, container: active, item: { data: item(4, null) }, previousIndex: 1, currentIndex: 2 } as never);
    await settle();
    confirm()!.close(true);
    await settle();
    const moved = await reply((url) => url === '/api/v1/boards/b1/backlog/moves', null, 204);
    expect(moved.request.body).toEqual({ issueId: 'i4', sprintId: 'sp1', afterId: 'i2', beforeId: null });
    await reply(backlogAsked, running());

    await click(page().querySelector('.close-sprint'));
    expect(dialogs()[0].componentInstance).toBeInstanceOf(CloseSprintDialog);
    await reply((url) => url === '/api/v1/boards/b1/sprints?state=open', { items: running().sprints.map((s) => s.sprint) });
    const pane = document.querySelector('app-close-sprint-dialog') as HTMLElement;
    expect(text(pane.querySelector('.summary'))).toBe('1 tickets done, 1 unfinished.');
    expect(text(pane.querySelector('mat-select'))).toBe('Sprint 25');
    await click(pane.querySelector('.confirm-close'));
    const closed = await reply((url, method) => url === '/api/v1/sprints/sp1/close' && method === 'POST', {
      sprint: { ...running().sprints[0].sprint, state: 'closed' },
      moved: 1,
      target: running().sprints[1].sprint,
    });
    expect(closed.request.headers.get('If-Match')).toBe('"1"');
    expect(closed.request.body).toEqual({ moveTo: 'sprint', sprintId: 'sp2' });
    await reply(backlogAsked, backlog());
  });

  it('closes the active sprint from its board, the unfinished tickets to a new sprint', async () => {
    const { page, click, reply, text, backlogAsked } = await open(null, activeCards());
    await click(page().querySelector('.sprint-head .close-sprint'));
    await reply((url) => url === '/api/v1/boards/b1/sprints?state=open', { items: [running().sprints[0].sprint] });
    const pane = document.querySelector('app-close-sprint-dialog') as HTMLElement;
    expect(text(pane.querySelector('.summary'))).toBe('1 tickets done, 1 unfinished.');
    // No future sprint: the backlog by default, or a new sprint.
    expect(pane.querySelector('.to-sprint')).toBeNull();
    await click(pane.querySelector('.to-new input'));
    await click(pane.querySelector('.confirm-close'));
    const closed = await reply((url) => url === '/api/v1/sprints/sp1/close', {
      sprint: { ...running().sprints[0].sprint, state: 'closed' },
      moved: 1,
      target: { ...running().sprints[1].sprint, id: 'sp3', name: 'PLAT Sprint 3' },
    });
    expect(closed.request.body).toEqual({ moveTo: 'new_sprint', sprintId: null });
    // Then the backlog.
    await reply(backlogAsked, backlog());
    expect(page().querySelector('app-backlog')).not.toBeNull();
  });

  it('tells a board without an active sprint, and lists the closed sprints with their scope', async () => {
    const { page, click, reply, text, backlogAsked, cardsAsked, dialogs } = await open(backlog());
    await click(page().querySelectorAll('.views button')[1]);
    await reply(cardsAsked, { columns: [], swimlanes: 'none', lanes: [], total: 0, unmapped: 0, truncated: false, sprint: null });
    expect(text(page().querySelector('.no-sprint span'))).toBe('No active sprint: start one from the backlog.');
    await click(page().querySelector('.no-sprint .to-backlog'));
    await reply(backlogAsked, backlog());

    await click(page().querySelector('.show-closed'));
    const closedSprint = {
      ...running().sprints[0].sprint,
      id: 'c1',
      name: 'PLAT Sprint 0',
      state: 'closed',
      completedCount: 1,
      completedEstimate: 8,
    };
    await reply((url) => url === '/api/v1/boards/b1/sprints?state=closed', { items: [closedSprint] });
    expect(text(page().querySelector('.closed-sprint .meta'))).toBe('8 of 11 points completed · 1 of 2 committed tickets');
    await click(page().querySelector('.see-scope'));
    expect(dialogs()[0].componentInstance).toBeInstanceOf(SprintScopeDialog);
    await reply((url) => url === '/api/v1/sprints/c1/scope', {
      sprint: closedSprint,
      hidden: 1,
      items: [
        { issueId: 'i1', key: 'PLAT-1', summary: 'Ticket 1', committed: true, addedLater: false, removed: false, completed: true, estimateAtStart: 8, estimateAtClose: 8 },
        { issueId: 'i2', key: 'PLAT-2', summary: 'Ticket 2', committed: true, addedLater: false, removed: false, completed: false, estimateAtStart: 3, estimateAtClose: 3 },
        { issueId: 'i4', key: 'PLAT-4', summary: 'Ticket 4', committed: false, addedLater: true, removed: true, completed: null, estimateAtStart: null, estimateAtClose: null },
      ],
    });
    const pane = document.querySelector('app-sprint-scope-dialog') as HTMLElement;
    expect(text(pane.querySelector('.totals'))).toBe('Committed: 2 tickets, 11 points · completed: 1 tickets, 8 points');
    const rows = Array.from(pane.querySelectorAll('tr.mat-mdc-row')).map((r) => Array.from(r.querySelectorAll('td')).map((c) => text(c)));
    expect(rows).toEqual([
      ['PLAT-1', 'Ticket 1', 'Committed', 'Done', '8'],
      ['PLAT-2', 'Ticket 2', 'Committed', 'Not done', '3'],
      ['PLAT-4', 'Ticket 4', 'Added later', 'Taken out', '–'],
    ]);
    expect(text(pane.querySelector('.hidden'))).toBe('1 more tickets you may not see.');
    expect(await axeViolations(pane)).toEqual([]);
  });

  it('passes the accessibility checks on the active sprint and in the close dialog', async () => {
    const { page, click, reply } = await open(null, activeCards());
    expect(await axeViolations(page())).toEqual([]);
    await click(page().querySelector('.sprint-head .close-sprint'));
    await reply((url) => url === '/api/v1/boards/b1/sprints?state=open', { items: running().sprints.map((s) => s.sprint) });
    expect(await axeViolations(document.querySelector('mat-dialog-container') as HTMLElement)).toEqual([]);
  });
});
