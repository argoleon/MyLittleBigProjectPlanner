/**
 * Tests of the board settings dialog (L06-US05), the API mocked at the HTTP
 * level and the real translations: a new board on a node with its name and
 * filter, a filter refused where it goes wrong; a board's columns
 * (statuses, limits, order) and quick filters changed, the API's field
 * errors placed on their fields, a deletion after confirmation, and
 * accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog, MatDialogRef } from '@angular/material/dialog';
import { provideRouter } from '@angular/router';
import { Board, ConfirmDialog, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { BoardSettingsData, BoardSettingsDialog, BoardSettingsOutcome } from './board-settings-dialog';

const BOARD: Board = {
  id: 'b1',
  node: { id: 'p1', key: 'PLAT', name: 'Identity platform', nodeType: 'project', archived: false },
  name: 'Platform board',
  kind: 'kanban',
  filter: 'project = PLAT',
  estimation: 'points',
  swimlanes: 'none',
  quickFilters: [{ label: 'My tickets', query: 'assignee = currentUser()' }],
  columns: [
    { id: 'c1', name: 'To do', statuses: ['todo'], wipLimit: null },
    { id: 'c2', name: 'In progress', statuses: ['in_progress'], wipLimit: 3 },
    { id: 'c3', name: 'Done', statuses: ['done'], wipLimit: null },
  ],
  canManage: true,
  version: 2,
};

const STATUSES = [
  { key: 'done', label: 'Done', category: 'done' },
  { key: 'in_progress', label: 'In progress', category: 'in_progress' },
  { key: 'in_review', label: 'In review', category: 'in_progress' },
  { key: 'todo', label: 'To do', category: 'todo' },
];

const NODES = {
  items: [
    { id: 'p1', type: 'project', key: 'PLAT', name: 'Identity platform', archived: false },
    { id: 'p2', type: 'project', key: 'OLD', name: 'Old project', archived: true },
    { id: 'v1', veiled: true, type: 'portfolio' },
  ],
  nextCursor: null,
};

describe('the board settings dialog', () => {
  async function open(data: BoardSettingsData) {
    TestBed.configureTestingModule({
      providers: [provideRouter([]), provideApi(), provideHttpClientTesting(), provideI18nTesting(), provideMaterialDefaults()],
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
    const ref: MatDialogRef<BoardSettingsDialog, BoardSettingsOutcome> = TestBed.inject(MatDialog).open(BoardSettingsDialog, { data });
    const closed: (BoardSettingsOutcome | undefined)[] = [];
    ref.beforeClosed().subscribe((outcome) => closed.push(outcome));
    await settle();
    if (data.board) await reply((url) => url === '/api/v1/boards/statuses?nodeId=p1', STATUSES);
    else await reply((url) => url === '/api/v1/nodes?limit=200', NODES);
    if (data.nodeId) await reply((url) => url === `/api/v1/nodes/${data.nodeId}`, { id: data.nodeId, type: 'project', key: 'NEW', name: 'New project', archived: false });
    const pane = () => document.querySelector('app-board-settings-dialog') as HTMLElement;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const texts = (selector: string) => Array.from(pane().querySelectorAll(selector)).map((e) => text(e));
    const click = async (element: Element | null | undefined) => {
      if (!element) throw new Error('nothing to click');
      (element as HTMLElement).click();
      await settle();
    };
    const type = async (selector: string, value: string, within: ParentNode = pane()) => {
      const input = within.querySelector(selector) as HTMLInputElement;
      input.value = value;
      input.dispatchEvent(new Event('input'));
      await settle();
    };
    const values = (selector: string) => Array.from(pane().querySelectorAll<HTMLInputElement>(selector)).map((i) => i.value);
    return { http, reply, settle, pane, text, texts, click, type, values, closed };
  }

  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify({ ignoreCancelled: true });
  });

  it('creates a board on a node with its name and filter, the rest by default', async () => {
    const { pane, click, type, reply, text, closed } = await open({});
    expect(text(pane().querySelector('h2'))).toBe('New board');
    expect(text(pane().querySelector('.note'))).toContain('a column per status category');
    expect(pane().querySelector('.column-row')).toBeNull();
    await click(pane().querySelector('mat-select'));
    const options = Array.from(document.querySelectorAll('mat-option')).map((o) => text(o));
    expect(options).toEqual(['Identity platform (PLAT)']);
    await click(document.querySelector('mat-option'));
    await type('input[formControlName="name"]', 'Platform board');
    await type('textarea', 'project = ');
    await click(pane().ownerDocument.querySelector('button.save'));
    await reply(
      (url, method) => url === '/api/v1/boards' && method === 'POST',
      { code: 'query.invalid', status: 400, title: 'Invalid', query: { reason: 'syntax', position: 10, length: 0, expected: ['a value'], found: null } },
      400,
    );
    expect(text(pane().querySelector('mat-error'))).toContain('a value');
    await type('textarea', 'project = PLAT');
    await click(pane().ownerDocument.querySelector('button.save'));
    const created = await reply((url, method) => url === '/api/v1/boards' && method === 'POST', BOARD);
    expect(created.request.body).toEqual({
      nodeId: 'p1',
      name: 'Platform board',
      filter: 'project = PLAT',
      estimation: 'points',
      swimlanes: 'none',
    });
    expect(closed).toEqual([BOARD]);
  });

  it('starts on the node asked for, even beyond the nodes listed', async () => {
    const { pane, click, text } = await open({ nodeId: 'p9' });
    expect(text(pane().querySelector('mat-select'))).toBe('New project (NEW)');
    await click(pane().querySelector('mat-select'));
    const options = Array.from(document.querySelectorAll('mat-option')).map((o) => text(o));
    expect(options).toEqual(['New project (NEW)', 'Identity platform (PLAT)']);
  });

  it('changes the columns and quick filters of a board, in order', async () => {
    const { pane, click, type, reply, texts, values, closed } = await open({ board: BOARD });
    expect(texts('h2')).toEqual(['Configure the board']);
    expect(values('.column-name input')).toEqual(['To do', 'In progress', 'Done']);
    expect(values('.column-wip input')).toEqual(['', '3', '']);
    // In progress down, after Done; then a column for the review.
    await click(pane().querySelectorAll('.column-row')[1].querySelector('button.down'));
    expect(values('.column-name input')).toEqual(['To do', 'Done', 'In progress']);
    await click(pane().querySelector('.add-column'));
    const rows = pane().querySelectorAll('.column-row');
    await type('.column-name input', 'Review', rows[3]);
    await type('.column-wip input', '2', rows[3]);
    await click(rows[3].querySelector('mat-select'));
    const options = Array.from(document.querySelectorAll('mat-option')).map((o) => o.textContent?.replace(/\s+/g, ' ').trim());
    expect(options).toEqual(['Done (in another column)', 'In progress (in another column)', 'In review', 'To do (in another column)']);
    await click(document.querySelectorAll('mat-option')[2]);
    // Closed by its own backdrop, not the dialog's.
    (document.querySelector('.cdk-overlay-transparent-backdrop') as HTMLElement).click();
    await new Promise((resolve) => setTimeout(resolve, 50));
    // The quick filter "Bugs".
    await click(pane().querySelector('.add-quick'));
    const quick = pane().querySelectorAll('.quick-row')[1];
    await type('.quick-label input', 'Bugs', quick);
    await type('.quick-query input', 'type = bug', quick);
    await click(pane().ownerDocument.querySelector('button.save'));
    const saved = await reply((url, method) => url === '/api/v1/boards/b1' && method === 'PUT', { ...BOARD, version: 3 });
    expect(saved.request.headers.get('If-Match')).toBe('"2"');
    expect(saved.request.body).toEqual({
      name: 'Platform board',
      filter: 'project = PLAT',
      estimation: 'points',
      swimlanes: 'none',
      columns: [
        { name: 'To do', statuses: ['todo'], wipLimit: null },
        { name: 'Done', statuses: ['done'], wipLimit: null },
        { name: 'In progress', statuses: ['in_progress'], wipLimit: 3 },
        { name: 'Review', statuses: ['in_review'], wipLimit: 2 },
      ],
      quickFilters: [
        { label: 'My tickets', query: 'assignee = currentUser()' },
        { label: 'Bugs', query: 'type = bug' },
      ],
    });
    expect(closed).toEqual([{ ...BOARD, version: 3 }]);
  });

  it('puts the API field errors on their fields', async () => {
    const { pane, click, reply, texts, http } = await open({ board: BOARD });
    await click(pane().querySelectorAll('.column-row')[2].querySelector('button.remove-column'));
    expect(pane().querySelectorAll('.column-row').length).toBe(2);
    await click(pane().ownerDocument.querySelector('button.save'));
    await reply(
      (url, method) => url === '/api/v1/boards/b1' && method === 'PUT',
      {
        code: 'validation_failed',
        status: 400,
        title: 'Invalid',
        errors: [
          { field: 'columns.1.statuses', code: 'duplicate' },
          { field: 'quickFilters.0.query', code: 'order_by_not_allowed' },
        ],
      },
      400,
    );
    expect(texts('mat-error')).toEqual(['This value appears twice.', 'A quick filter cannot sort: remove ORDER BY.']);
    expect(pane().querySelector('[role="alert"]')).toBeNull();
    // Still wrong: not sent again until changed.
    await click(pane().ownerDocument.querySelector('button.save'));
    expect(http.match(() => true)).toEqual([]);
  });

  it('tells a name taken on the name, and a board changed elsewhere above the form', async () => {
    const { pane, click, reply, text, type } = await open({ board: BOARD });
    await click(pane().ownerDocument.querySelector('button.save'));
    await reply(
      (url) => url === '/api/v1/boards/b1',
      { code: 'board.name_taken', status: 409, title: 'Taken', errors: [{ field: 'name', code: 'taken' }] },
      409,
    );
    expect(text(pane().querySelector('mat-form-field mat-error'))).toBe('This value is already used.');
    // Changed elsewhere.
    await type('input[formControlName="name"]', 'Platform');
    await click(pane().ownerDocument.querySelector('button.save'));
    await reply((url) => url === '/api/v1/boards/b1', { code: 'precondition_failed', status: 412, title: 'Changed' }, 412);
    expect(text(pane().querySelector('[role="alert"]'))).toContain('Someone changed this board');
  });

  it('deletes the board once confirmed', async () => {
    const { pane, click, reply, closed, settle } = await open({ board: BOARD });
    await click(pane().ownerDocument.querySelector('button.delete'));
    const dialogs = TestBed.inject(MatDialog).openDialogs;
    const confirm = dialogs.find((d) => d.componentInstance instanceof ConfirmDialog);
    expect(confirm).toBeDefined();
    confirm!.close(true);
    await settle();
    const deleted = await reply((url, method) => url === '/api/v1/boards/b1' && method === 'DELETE', null);
    expect(deleted.request.headers.get('If-Match')).toBe('"2"');
    expect(closed).toEqual(['deleted']);
  });

  it('passes the accessibility checks', async () => {
    const { pane } = await open({ board: BOARD });
    expect(await axeViolations(pane())).toEqual([]);
  });
});
