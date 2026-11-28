/**
 * Tests of the Kanban board (L06-US05), the API mocked at the HTTP level,
 * the stream faked and the real translations: each card in the column of
 * its status, a column over its limit highlighted, quick filters and
 * swimlanes asked of the API, a card dropped in a column or moved from its
 * menu with the keyboard, a refused move put back with its reason, a
 * transition with fields or several to choose from, others' moves read as
 * they happen, failures and accessibility.
 */
import { CdkDropList } from '@angular/cdk/drag-drop';
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { By } from '@angular/platform-browser';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { Board, BoardCard, BoardCards, EVENT_SOURCE, EventSourceLike, Issue, IssueTransition, Realtime, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { BoardPage, REALTIME_DELAY_MS } from './board-page';
import { ChooseTransitionDialog } from './choose-transition-dialog';
import { TransitionDialog } from '../issues/transition-dialog';

const BOARD: Board = {
  id: 'b1',
  node: { id: 'p1', key: 'PLAT', name: 'Identity platform', nodeType: 'project', archived: false },
  name: 'Platform board',
  kind: 'kanban',
  filter: 'project = PLAT',
  estimation: 'points',
  swimlanes: 'none',
  quickFilters: [
    { label: 'My tickets', query: 'assignee = currentUser()' },
    { label: 'Bugs', query: 'type = bug' },
  ],
  columns: [
    { id: 'c-todo', name: 'To do', statuses: ['todo'], wipLimit: null },
    { id: 'c-progress', name: 'In progress', statuses: ['in_progress'], wipLimit: 3 },
    { id: 'c-review', name: 'Review', statuses: ['in_review'], wipLimit: null },
    { id: 'c-done', name: 'Done', statuses: ['done'], wipLimit: null },
  ],
  canManage: true,
  canPlan: true,
  version: 2,
};

const STATUS = {
  todo: { id: 's1', key: 'todo', label: 'To do', category: 'todo' as const },
  in_progress: { id: 's2', key: 'in_progress', label: 'In progress', category: 'in_progress' as const },
  in_review: { id: 's3', key: 'in_review', label: 'In review', category: 'in_progress' as const },
  done: { id: 's4', key: 'done', label: 'Done', category: 'done' as const },
};

function card(n: number, status: keyof typeof STATUS, overrides: Partial<BoardCard> = {}): BoardCard {
  return {
    id: `i${n}`,
    key: `PLAT-${n}`,
    summary: `Ticket ${n}`,
    projectId: 'p1',
    type: { id: 't1', key: 'story', label: 'Story', labelTranslations: { en: 'Story' }, icon: 'bookmark', color: 'story', isSubtask: false, parentIds: [] },
    status: STATUS[status],
    priority: 'medium',
    assignee: null,
    estimate: null,
    epic: null,
    blockedBy: [],
    version: 1,
    ...overrides,
  };
}

const CAMILLE = { id: 'a2', name: 'Camille Martin' };

function cards(overrides: Partial<BoardCards> = {}): BoardCards {
  return {
    columns: [
      { id: 'c-todo', name: 'To do', statuses: ['todo'], wipLimit: null, count: 1, wip: 'unlimited' },
      { id: 'c-progress', name: 'In progress', statuses: ['in_progress'], wipLimit: 3, count: 4, wip: 'exceeded' },
      { id: 'c-review', name: 'Review', statuses: ['in_review'], wipLimit: null, count: 0, wip: 'unlimited' },
      { id: 'c-done', name: 'Done', statuses: ['done'], wipLimit: null, count: 1, wip: 'unlimited' },
    ],
    swimlanes: 'none',
    lanes: [
      {
        id: null,
        key: null,
        label: null,
        cells: [
          [card(1, 'todo', { assignee: CAMILLE, estimate: 5, blockedBy: ['PLAT-9'] })],
          [card(2, 'in_progress'), card(3, 'in_progress'), card(4, 'in_progress'), card(5, 'in_progress')],
          [],
          [card(6, 'done')],
        ],
      },
    ],
    total: 6,
    unmapped: 1,
    truncated: false,
    ...overrides,
  };
}

const ISSUE: Issue = {
  id: 'i2',
  key: 'PLAT-2',
  project: { id: 'p1', key: 'PLAT', name: 'Identity platform' },
  type: { id: 't1', key: 'story', label: 'Story', labelTranslations: { en: 'Story' }, icon: 'bookmark', color: 'story', isSubtask: false, parentIds: [] },
  status: STATUS.in_progress,
  summary: 'Ticket 2',
  description: null,
  descriptionHtml: null,
  priority: 'medium',
  reporter: null,
  assignee: null,
  startDate: null,
  dueDate: null,
  fields: [],
  parentId: null,
  createdAt: '2026-11-06T09:12:00Z',
  updatedAt: '2026-11-06T18:20:00Z',
  version: 1,
};

const REVIEW: IssueTransition = {
  id: 'tr-review',
  name: 'Send to review',
  toStatus: STATUS.in_review,
  fields: [{ fieldId: 'f-points', key: 'story_points', label: 'Points', kind: 'number', mode: 'visible', options: {}, value: null } as never],
};
const VALIDATE: IssueTransition = { id: 'tr-validate', name: 'Validate', toStatus: STATUS.done, fields: [] };
const CANCEL: IssueTransition = { id: 'tr-cancel', name: 'Cancel', toStatus: { id: 's5', key: 'cancelled', label: 'Cancelled', category: 'done' }, fields: [] };

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

describe('the Kanban board', () => {
  async function open(board: Board | null = BOARD, first: BoardCards = cards()) {
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
    const reply = async (
      match: (url: string, method: string) => boolean,
      body: object | null,
      status = 200,
    ): Promise<TestRequest> => {
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
    if (board) await reply((url, method) => url === '/api/v1/boards/b1' && method === 'GET', board);
    else await reply((url) => url === '/api/v1/boards/b1', { code: 'not_found', status: 404, title: 'Not found' }, 404);
    const cardsAsked = (query = '?swimlanes=none') => (url: string, method: string) => url === `/api/v1/boards/b1/cards${query}` && method === 'GET';
    if (board) await reply(cardsAsked(), first);
    const page = () => harness.routeNativeElement as HTMLElement;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const texts = (selector: string) => Array.from(page().querySelectorAll(selector)).map((e) => text(e));
    const click = async (element: Element | null | undefined) => {
      if (!element) throw new Error('nothing to click');
      (element as HTMLElement).click();
      await settle();
    };
    /** The keys of each column. */
    const columns = () =>
      Array.from(page().querySelectorAll('.lane:first-of-type .cell')).map((cell) =>
        Array.from(cell.querySelectorAll('app-board-card .key')).map((k) => text(k)),
      );
    const dialogs = () => TestBed.inject(MatDialog).openDialogs;
    return { http, harness, page, reply, settle, text, texts, click, columns, cardsAsked, source, dialogs };
  }

  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify({ ignoreCancelled: true });
  });

  it('shows each ticket in the column of its status, and a column over its limit', async () => {
    const { page, texts, text, columns } = await open();
    expect(text(page().querySelector('h1'))).toBe('Platform board');
    expect(text(page().querySelector('.subtitle'))).toBe('Kanban · 6 tickets · 1 without a column');
    expect(text(page().querySelector('.filter-query'))).toBe('project = PLAT');
    expect(texts('.column-name')).toEqual(['To do 1', 'In progress 4', 'Review 0', 'Done 1']);
    expect(columns()).toEqual([['PLAT-1'], ['PLAT-2', 'PLAT-3', 'PLAT-4', 'PLAT-5'], [], ['PLAT-6']]);
    const progress = page().querySelectorAll('.column-head')[1];
    expect(progress.getAttribute('data-wip')).toBe('exceeded');
    expect(text(progress.querySelector('.wip'))).toBe('Limit exceeded · 4 / 3');
    expect(page().querySelectorAll('.cell')[1].getAttribute('data-wip')).toBe('exceeded');
    // The card: its link, blocker, estimate and assignee.
    const first = page().querySelector('app-board-card')!;
    expect(first.querySelector('a.summary')?.getAttribute('href')).toBe('/issues/PLAT-1');
    expect(text(first.querySelector('.blocked'))).toBe('block Blocked by PLAT-9');
    expect(first.querySelector('.estimate')?.getAttribute('aria-label')).toBe('5 points');
    expect(text(first.querySelector('ui-avatar'))).toBe('CMCamille Martin');
    expect(first.getAttribute('aria-label')).toBe('PLAT-1 Ticket 1');
    expect(page().querySelector('.configure')).not.toBeNull();
    expect(page().querySelector('.new-ticket')).not.toBeNull();
  });

  it('asks the API for the quick filters toggled on and the swimlanes chosen', async () => {
    const { page, click, reply, cardsAsked, texts } = await open();
    const chips = page().querySelectorAll('mat-chip-option');
    expect(Array.from(chips).map((c) => c.textContent?.trim())).toEqual(['My tickets', 'Bugs']);
    await click(chips[1].querySelector('button') ?? chips[1]);
    await reply(cardsAsked('?quick=1&swimlanes=none'), cards({ total: 1 }));
    await click(chips[0].querySelector('button') ?? chips[0]);
    await reply(cardsAsked('?quick=0%2C1&swimlanes=none'), cards({ total: 1 }));

    const toggles = page().querySelectorAll('mat-button-toggle button');
    await click(toggles[1]);
    await reply(
      cardsAsked('?quick=0%2C1&swimlanes=assignee'),
      cards({
        swimlanes: 'assignee',
        lanes: [
          { id: 'a2', key: null, label: 'Camille Martin', cells: [[card(1, 'todo', { assignee: CAMILLE })], [], [], []] },
          { id: null, key: null, label: null, cells: [[], [card(2, 'in_progress')], [], []] },
        ],
      }),
    );
    expect(texts('.lane-title')).toEqual(['CMCamille Martin Camille Martin 1', '?Unassigned Unassigned 1']);
    await click(toggles[2]);
    await reply(
      cardsAsked('?quick=0%2C1&swimlanes=epic'),
      cards({
        swimlanes: 'epic',
        lanes: [{ id: 'e1', key: 'PLAT-100', label: 'Unified authentication', cells: [[card(1, 'todo')], [], [], []] }],
      }),
    );
    expect(texts('.lane-title')).toEqual(['PLAT-100 Unified authentication 1']);
  });

  it('moves a card from its menu with the keyboard', async () => {
    const { page, click, reply, cardsAsked, columns } = await open();
    const menu = page().querySelectorAll('app-board-card')[1].querySelector('button.move') as HTMLElement;
    expect(menu.getAttribute('aria-label')).toBe('Move PLAT-2');
    await click(menu);
    const targets = Array.from(document.querySelectorAll('.move-to')).map((b) => b.textContent?.trim());
    expect(targets).toEqual(['Move to To do', 'Move to Review', 'Move to Done']);
    await click(document.querySelectorAll('.move-to')[2]);
    const moved = await reply((url, method) => url === '/api/v1/boards/b1/moves' && method === 'POST', {
      outcome: 'moved',
      issue: { ...ISSUE, status: STATUS.done, version: 2 },
      transitions: [],
    });
    expect(moved.request.headers.get('If-Match')).toBe('"1"');
    expect(moved.request.body).toEqual({ issueId: 'i2', columnId: 'c-done' });
    const after = cards();
    after.lanes[0].cells[1] = after.lanes[0].cells[1].slice(1);
    after.lanes[0].cells[3] = [...after.lanes[0].cells[3], card(2, 'done', { version: 2 })];
    await reply(cardsAsked(), after);
    expect(columns()[3]).toEqual(['PLAT-6', 'PLAT-2']);
  });

  it('moves a dropped card at once, and puts it back with the reason when refused', async () => {
    const { page, harness, reply, cardsAsked, columns, settle, text } = await open();
    const lists = harness.fixture.debugElement.queryAll(By.directive(CdkDropList)).map((d) => d.injector.get(CdkDropList));
    const [todo, , review] = lists;
    const dragged = card(1, 'todo', { assignee: CAMILLE, estimate: 5, blockedBy: ['PLAT-9'] });
    review.dropped.emit({
      previousContainer: todo,
      container: review,
      item: { data: dragged },
      previousIndex: 0,
      currentIndex: 0,
      isPointerOverContainer: true,
      distance: { x: 300, y: 0 },
      dropPoint: { x: 600, y: 200 },
      event: new MouseEvent('mouseup'),
    } as never);
    await settle();
    // Already in its new column while the move runs.
    expect(columns()[0]).toEqual([]);
    expect(columns()[2]).toEqual(['PLAT-1']);
    await reply((url, method) => url === '/api/v1/boards/b1/moves' && method === 'POST', { code: 'board.no_transition', status: 409, title: 'No transition' }, 409);
    expect(text(page().querySelector('[role="alert"] span'))).toBe('PLAT-1 went back: no transition of its workflow leads from To do to Review.');
    await reply(cardsAsked(), cards());
    expect(columns()[0]).toEqual(['PLAT-1']);
    expect(columns()[2]).toEqual([]);

    // A transition the member may not run.
    review.dropped.emit({ previousContainer: todo, container: review, item: { data: dragged } } as never);
    await reply((url) => url === '/api/v1/boards/b1/moves', { code: 'permission.denied', status: 403, title: 'Denied', detail: 'the transition is restricted' }, 403);
    expect(text(page().querySelector('[role="alert"] span'))).toBe('PLAT-1 went back: you may not run the transition to Review.');
    await reply(cardsAsked(), cards());
    // Changed by someone else meanwhile.
    review.dropped.emit({ previousContainer: todo, container: review, item: { data: dragged } } as never);
    await reply((url) => url === '/api/v1/boards/b1/moves', { code: 'precondition_failed', status: 412, title: 'Changed' }, 412);
    expect(text(page().querySelector('[role="alert"]'))).toContain('PLAT-1 changed in the meantime');
    await reply(cardsAsked(), cards());
    // Dropped back in its own column: nothing asked.
    todo.dropped.emit({ previousContainer: todo, container: todo, item: { data: dragged } } as never);
    await settle();
  });

  it('reorders the cards of a column, dragged or with the keyboard (the board rank)', async () => {
    const { harness, page, reply, cardsAsked, columns, settle, click } = await open();
    const lists = harness.fixture.debugElement.queryAll(By.directive(CdkDropList)).map((d) => d.injector.get(CdkDropList));
    const progress = lists[1];
    progress.dropped.emit({ previousContainer: progress, container: progress, item: { data: card(2, 'in_progress') }, previousIndex: 0, currentIndex: 2 } as never);
    await settle();
    expect(columns()[1]).toEqual(['PLAT-3', 'PLAT-4', 'PLAT-2', 'PLAT-5']);
    const isRank = (url: string, method: string) => url === '/api/v1/boards/b1/rank' && method === 'POST';
    const ranked = await reply(isRank, null, 204);
    expect(ranked.request.body).toEqual({ issueId: 'i2', afterId: 'i4', beforeId: null });
    await reply(cardsAsked(), cards());

    const third = page().querySelectorAll('.cell')[1].querySelectorAll('app-board-card')[2];
    await click(third.querySelector('button.move'));
    expect(Array.from(document.querySelectorAll('.move-up, .move-down')).map((b) => b.textContent?.trim())).toEqual(['Move up', 'Move down']);
    await click(document.querySelector('.move-down'));
    const down = await reply(isRank, null, 204);
    expect(down.request.body).toEqual({ issueId: 'i4', afterId: 'i5', beforeId: null });
    await reply(cardsAsked(), cards());

    // Alone in its column: no moving up or down.
    await click(page().querySelectorAll('.cell')[0].querySelector('button.move'));
    expect(document.querySelector('.move-up, .move-down')).toBeNull();
  });

  it('opens the form of a transition with fields, and asks which when several lead there', async () => {
    const { page, click, reply, cardsAsked, dialogs, settle } = await open();
    await click(page().querySelectorAll('app-board-card')[1].querySelector('button.move'));
    await click(document.querySelectorAll('.move-to')[1]);
    await reply((url) => url === '/api/v1/boards/b1/moves', { outcome: 'screen', issue: ISSUE, transitions: [REVIEW] });
    expect(dialogs().length).toBe(1);
    expect(dialogs()[0].componentInstance).toBeInstanceOf(TransitionDialog);
    dialogs()[0].close();
    await settle();
    await reply(cardsAsked(), cards());

    await click(page().querySelectorAll('app-board-card')[1].querySelector('button.move'));
    await click(document.querySelectorAll('.move-to')[2]);
    await reply((url) => url === '/api/v1/boards/b1/moves', { outcome: 'choose', issue: ISSUE, transitions: [VALIDATE, CANCEL] });
    expect(dialogs()[0].componentInstance).toBeInstanceOf(ChooseTransitionDialog);
    const choices = Array.from(document.querySelectorAll('.choice')).map((c) => c.textContent?.trim());
    expect(choices).toEqual(['Validate → Done', 'Cancel → Cancelled']);
    await click(document.querySelectorAll('.choice')[1]);
    const picked = await reply((url) => url === '/api/v1/boards/b1/moves', {
      outcome: 'moved',
      issue: { ...ISSUE, status: STATUS.done, version: 2 },
      transitions: [],
    });
    expect(picked.request.body).toEqual({ issueId: 'i2', columnId: 'c-done', transitionId: 'tr-cancel' });
    await reply(cardsAsked(), cards());
  });

  it('reads the cards again when others change tickets', async () => {
    const { source, settle, http, reply, cardsAsked } = await open();
    source.emit('issue', { topic: 'issue', kind: 'issue.transitioned', issueId: 'i3', issueKey: 'PLAT-3', actorId: 'a9' });
    source.emit('issue', { topic: 'issue', kind: 'issue.updated', issueId: 'i4', issueKey: 'PLAT-4', actorId: 'a9' });
    source.emit('notifications');
    await settle();
    expect(http.match((r) => r.url.includes('/cards')).length).toBe(0);
    await new Promise((resolve) => setTimeout(resolve, REALTIME_DELAY_MS + 50));
    await settle();
    // One read for the burst.
    await reply(cardsAsked(), cards());
    expect(http.match((r) => r.url.includes('/cards')).length).toBe(0);
  });

  it('shows the truncated board, the more button, and a read-only archived board', async () => {
    const many = Array.from({ length: 23 }, (_, i) => card(10 + i, 'todo'));
    const { page, texts, click, text } = await open(
      { ...BOARD, canManage: false, canPlan: false, node: { ...BOARD.node, archived: true } },
      cards({ truncated: true, lanes: [{ id: null, key: null, label: null, cells: [many, [], [], []] }] }),
    );
    expect(text(page().querySelector('[role="status"].warning'))).toContain('first 1,000 tickets');
    expect(page().querySelectorAll('.cell')[0].querySelectorAll('app-board-card').length).toBe(20);
    expect(texts('.more')).toEqual(['Show 3 more tickets']);
    await click(page().querySelector('.more'));
    expect(page().querySelectorAll('.cell')[0].querySelectorAll('app-board-card').length).toBe(23);
    expect(page().querySelector('.configure')).toBeNull();
    expect(page().querySelector('.new-ticket')).toBeNull();
    expect(page().querySelector('button.move')).toBeNull();
    expect(text(page().querySelector('.subtitle'))).toContain('Archived: read-only');
  });

  it('tells when the board cannot be read', async () => {
    const { page } = await open(null);
    expect(page().querySelector('ui-error-state')).not.toBeNull();
  });

  it('passes the accessibility checks', async () => {
    const { page } = await open();
    expect(await axeViolations(page())).toEqual([]);
  });
});
