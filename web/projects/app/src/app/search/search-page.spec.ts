/**
 * Tests of the search screen (L06-US01), the API mocked at the HTTP level
 * and the real translations: the query of the address run and its results
 * with their total, Enter running a query, a refused query shown at its
 * position, sorting a column sorting the query, the next page loaded
 * through the cursor, the columns of the member's choice, empty and failed
 * searches, and accessibility; a saved filter opened, saved and gone
 * (L06-US04).
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { Router, provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { SavedFilter, SearchResult, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { SearchPage } from './search-page';

function result(n: number, overrides: Partial<SearchResult> = {}): SearchResult {
  return {
    id: `i${n}`,
    key: `PLAT-${n}`,
    summary: `Ticket ${n}`,
    project: { id: 'p1', key: 'PLAT', name: 'Identity platform' },
    type: { id: 't1', key: 'task', label: 'Task', labelTranslations: { en: 'Task' }, icon: 'task_alt', color: 'task', isSubtask: false, parentIds: [] },
    status: { id: 's1', key: 'todo', label: 'To do', category: 'todo' },
    priority: 'high',
    assignee: { id: 'a1', name: 'Lucas Bernard' },
    reporter: { id: 'a2', name: 'Camille Martin' },
    startDate: null,
    dueDate: '2026-11-20',
    parentId: null,
    createdAt: '2026-11-06T09:12:00Z',
    updatedAt: '2026-11-06T18:20:00Z',
    ...overrides,
  };
}

/** What the API suggests after `<field> IN (`: the builder's lists. */
const LISTS: Record<string, object[]> = {
  'status IN (': [
    { kind: 'value', text: 'todo', label: 'To do', detail: 'todo' },
    { kind: 'value', text: 'in_progress', label: 'In progress', detail: 'in_progress' },
  ],
};

/** The tree of `status IN (<keys>)`, as `POST /search/parse` returns it. */
function statusTree(query: string, ...keys: string[]) {
  return {
    query,
    filter: { kind: 'clause', field: 'status', operator: 'IN', values: keys.map((value) => ({ kind: 'text', value })) },
    orderBy: [],
  };
}

/** A filter shared with a group, of another member. */
const LATE: SavedFilter = {
  id: 'f1',
  name: 'Late',
  icon: 'schedule',
  query: 'due < now()',
  owner: { accountId: 'a2', displayName: 'Camille Martin', member: true },
  sharing: { all: false, groups: [{ id: 'g1', name: 'Platform' }] },
  mine: false,
  canEdit: false,
  canDelete: false,
  version: 1,
};

describe('the search screen', () => {
  /**
   * The screen at `url`, in the query mode unless told (`null`: never chosen
   * yet); `filters`: the saved filters the member sees.
   */
  async function open(url: string, mode: 'query' | 'builder' | null = 'query', filters: readonly SavedFilter[] = []) {
    localStorage.clear();
    if (mode) localStorage.setItem('mlbpp.search.mode', mode);
    TestBed.configureTestingModule({
      providers: [
        provideRouter([{ path: 'search', component: SearchPage }], withComponentInputBinding()),
        provideApi(),
        provideHttpClientTesting(),
        provideI18nTesting(),
        provideMaterialDefaults(),
      ],
    });
    const http = TestBed.inject(HttpTestingController);
    // The table draws the rows of the viewport's range on an animation frame.
    // The editor's suggestions are answered with none; the saved filters
    // with `filters`.
    const settle = async () => {
      for (let i = 0; i < 4; i++) {
        await new Promise((resolve) => setTimeout(resolve, 20));
        for (const asked of http.match((r) => r.url === '/api/v1/search/suggestions')) {
          const query = (asked.request.body as { query: string }).query;
          asked.flush({ from: 0, to: 0, suggestions: LISTS[query] ?? [] });
        }
        for (const asked of http.match((r) => r.url === '/api/v1/saved-filters' && r.method === 'GET')) {
          asked.flush({ items: filters });
        }
        TestBed.tick();
      }
    };
    const searched = async (body: object | null, status = 200): Promise<TestRequest> => {
      let request: TestRequest | undefined;
      for (let i = 0; i < 50 && !request; i++) {
        [request] = http.match((r) => r.url === '/api/v1/search' && r.method === 'POST');
        if (!request) await new Promise((resolve) => setTimeout(resolve, 20));
      }
      if (!request) throw new Error('no search request');
      request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error' });
      await settle();
      return request;
    };
    const parsed = async (body: object, status = 200): Promise<TestRequest> => {
      let request: TestRequest | undefined;
      for (let i = 0; i < 50 && !request; i++) {
        [request] = http.match((r) => r.url === '/api/v1/search/parse');
        if (!request) await new Promise((resolve) => setTimeout(resolve, 20));
      }
      if (!request) throw new Error('no parse request');
      request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error' });
      await settle();
      return request;
    };
    const replied = async (match: (url: string, method: string) => boolean, body: object | null, status = 200): Promise<TestRequest> => {
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
    const harness = await RouterTestingHarness.create();
    await harness.navigateByUrl(url);
    await settle();
    const root = () => document.body;
    const page = () => root().querySelector('app-search-page')!;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const texts = (selector: string) => Array.from(page().querySelectorAll(selector)).map((e) => text(e));
    const field = () => page().querySelector('textarea') as HTMLTextAreaElement;
    const click = async (element: Element | null | undefined) => {
      (element as HTMLElement).click();
      await settle();
    };
    return { http, harness, settle, searched, parsed, replied, root, page, text, texts, field, click };
  }

  it('runs the query of the address and shows the results with their total', async () => {
    const { searched, page, text, texts, field } = await open('/search?q=status%20%3D%20todo');
    const request = await searched({ items: [result(1), result(2, { priority: 'low', assignee: null })], nextCursor: null, total: 2 });
    expect(request.request.body).toEqual({ query: 'status = todo', limit: 50 });
    expect(field().value).toBe('status = todo');
    expect(text(page().querySelector('.subtitle'))).toBe('Results: 2');
    expect(texts('th:not(.select)')).toEqual(['Key', 'Summary', 'Type', 'Status', 'Assignee', 'Priority', 'Due']);
    expect(texts('td.key a')).toEqual(['PLAT-1', 'PLAT-2']);
    expect(page().querySelector('td.key a')?.getAttribute('href')).toBe('/issues/PLAT-1');
    expect(texts('tr.mat-mdc-row')[0]).toContain('Lucas Bernard');
    const table = page().querySelector('table')!;
    expect(table.getAttribute('aria-rowcount')).toBe('3');
    expect(Array.from(page().querySelectorAll('tr.mat-mdc-row')).map((r) => r.getAttribute('aria-rowindex'))).toEqual(['2', '3']);
    expect(await axeViolations(page())).toEqual([]);
  });

  it('runs a query on Enter and keeps it in the address; Shift+Enter starts a new line', async () => {
    const { searched, settle, field } = await open('/search');
    const first = await searched({ items: [], nextCursor: null, total: 0 });
    expect(first.request.body).toEqual({ query: '', limit: 50 });

    field().value = 'assignee = currentUser()';
    field().dispatchEvent(new Event('input'));
    field().dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', shiftKey: true }));
    await settle();
    field().dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }));
    await settle();
    const again = await searched({ items: [result(3)], nextCursor: null, total: 1 });
    expect(again.request.body).toEqual({ query: 'assignee = currentUser()', limit: 50 });
    expect(decodeURIComponent(TestBed.inject(Router).url)).toBe('/search?q=assignee = currentUser()');
    expect(TestBed.inject(HttpTestingController).match(() => true)).toEqual([]);
  });

  it('shows a refused query at its position, the part concerned selected', async () => {
    const { searched, page, text, field } = await open('/search?q=status%20%3D');
    await searched(
      {
        type: 'urn:mlbpp:problem:query.invalid',
        title: 'The query cannot be run',
        status: 400,
        code: 'query.invalid',
        detail: 'Expected a value at position 8, found the end of the query.',
        query: { reason: 'syntax', position: 8, length: 0, expected: ['a value'], found: null },
      },
      400,
    );
    const alert = page().querySelector('#query-error[role="alert"]');
    expect(text(alert?.querySelector('.reason'))).toBe('Character 9: expected a value, found the end of the query.');
    expect(field().getAttribute('aria-describedby')).toBe('query-error');
    expect(field().getAttribute('aria-invalid')).toBe('true');
    expect(document.activeElement).toBe(field());
    expect([field().selectionStart, field().selectionEnd]).toEqual([8, 8]);
    expect(page().querySelector('table')).toBeNull();
    expect(await axeViolations(page())).toEqual([]);
  });

  it('names the field and what it takes when an operator does not fit', async () => {
    const { searched, page, text } = await open('/search?q=text%20%3D%20outage');
    await searched(
      {
        status: 400,
        code: 'query.invalid',
        query: { reason: 'operator_not_allowed', position: 0, length: 13, found: null, field: 'text', allowed: ['~', '!~'] },
      },
      400,
    );
    expect(text(page().querySelector('#query-error .reason'))).toBe('The field “text” does not take this operator; it takes: ~, !~.');
    expect(text(page().querySelector('#query-error mark'))).toBe('text = outage');
  });

  it('sorts the query when a column is sorted', async () => {
    const { searched, page, click } = await open('/search?q=status%20%3D%20todo%20ORDER%20BY%20priority%20DESC');
    await searched({ items: [result(1)], nextCursor: null, total: 1 });
    const priority = Array.from(page().querySelectorAll('th')).find((th) => th.textContent?.includes('Priority'))!;
    expect(priority.getAttribute('aria-sort')).toBe('descending');
    const key = Array.from(page().querySelectorAll('th')).find((th) => th.textContent?.includes('Key'))!;
    await click(key.querySelector('.mat-sort-header-container'));
    const sorted = await searched({ items: [result(1)], nextCursor: null, total: 1 });
    expect(sorted.request.body).toEqual({ query: 'status = todo ORDER BY key ASC', limit: 50 });
  });

  it('loads the next page through the cursor as its end comes into view', async () => {
    const { searched, page, settle } = await open('/search?q=ORDER%20BY%20key');
    const first = Array.from({ length: 12 }, (_, i) => result(i + 1));
    await searched({ items: first, nextCursor: 'c2', total: 14 });
    const next = await searched({ items: [result(13), result(14)], nextCursor: null, total: null });
    expect(next.request.body).toEqual({ query: 'ORDER BY key', limit: 50, cursor: 'c2' });
    await settle();
    expect(page().querySelector('table')?.getAttribute('aria-rowcount')).toBe('15');
    expect(page().querySelector('.subtitle')?.textContent?.trim()).toBe('Results: 14');
    expect(TestBed.inject(HttpTestingController).match(() => true)).toEqual([]);
  });

  it('shows the columns the member chooses, kept in this browser; the key always', async () => {
    const { searched, page, root, texts, click } = await open('/search');
    await searched({ items: [result(1)], nextCursor: null, total: 1 });
    await click(page().querySelector('button.columns'));
    const choices = Array.from(root().querySelectorAll('button.column-choice'));
    expect(choices.map((c) => [c.textContent?.trim().replace(/^\S+\s+/, ''), c.getAttribute('aria-checked')])).toContainEqual([
      'Reporter',
      'false',
    ]);
    expect((choices[0] as HTMLButtonElement).disabled).toBe(true);
    await click(choices.find((c) => c.textContent?.includes('Reporter')));
    await click(choices.find((c) => c.textContent?.includes('Type')));
    expect(texts('th:not(.select)')).toEqual(['Key', 'Summary', 'Status', 'Assignee', 'Reporter', 'Priority', 'Due']);
    expect(JSON.parse(localStorage.getItem('mlbpp.search.columns') ?? '[]')).toEqual([
      'key',
      'summary',
      'status',
      'assignee',
      'reporter',
      'priority',
      'due',
    ]);
  });

  it('exports the results with the columns displayed and the query run', async () => {
    const { searched, page, click, http, settle } = await open('/search?q=status%20%3D%20todo');
    await searched({ items: [result(1)], nextCursor: null, total: 1 });
    await click(page().querySelector('button.export'));
    await click(document.querySelector('button.csv'));
    let asked: TestRequest | undefined;
    for (let i = 0; i < 50 && !asked; i++) {
      [asked] = http.match((r) => r.url === '/api/v1/exports' && r.method === 'POST');
      if (!asked) await new Promise((resolve) => setTimeout(resolve, 20));
    }
    expect(asked!.request.body).toEqual({
      query: 'status = todo',
      columns: ['key', 'summary', 'type', 'status', 'assignee', 'priority', 'due'],
      format: 'csv',
      name: 'search',
    });
    asked!.flush({ id: 'e1', fileName: 'search.csv', format: 'csv', status: 'queued', downloadPath: null }, { status: 202, statusText: 'Accepted' });
    await settle();
    expect(page().querySelector('app-export-menu [role="status"]')).not.toBeNull();
  });

  it('selects tickets, then all the results of the query, for a bulk action', async () => {
    const { searched, page, root, text, texts, click, replied } = await open('/search?q=status%20%3D%20todo');
    const first = Array.from({ length: 3 }, (_, i) => result(i + 1));
    await searched({ items: first, nextCursor: null, total: 120 });
    expect(page().querySelector('.bulk-bar')).toBeNull();
    // One row ticked: the bar, with its count.
    await click(page().querySelector('td.select mat-checkbox input'));
    expect(text(page().querySelector('.bulk-count'))).toBe('1 selected');
    expect(page().querySelector('button.select-all')).toBeNull();
    // Every loaded row ticked: all the results offered, then chosen.
    await click(page().querySelector('th.select mat-checkbox input'));
    expect(text(page().querySelector('.bulk-count'))).toBe('3 selected');
    await click(page().querySelector('button.select-all'));
    expect(text(page().querySelector('.bulk-count'))).toBe('120 selected');
    expect(await axeViolations(page())).toEqual([]);
    // Change status: the dialog asks the options of the query.
    await click(page().querySelector('button.bulk-status'));
    const asked = await replied((url, method) => url === '/api/v1/bulk-actions/options' && method === 'POST', {
      total: 120,
      background: true,
      projectKey: 'PLAT',
      fields: [],
      statuses: [{ key: 'done', label: 'Done', category: 'done', reachable: 120 }],
    });
    expect(asked.request.body).toEqual({ query: 'status = todo' });
    await click(root().querySelector('mat-dialog-container mat-radio-button input'));
    await click(root().querySelector('mat-dialog-container button.confirm'));
    const run = await replied((url, method) => url === '/api/v1/bulk-actions' && method === 'POST', {
      id: 'b1',
      kind: 'transition',
      status: 'running',
      background: true,
      total: 120,
      counts: { changed: 0, unchanged: 0, refused: 0, withParent: 0, waiting: 120 },
      items: [],
      createdAt: '2026-11-19T09:00:00Z',
      completedAt: null,
      expiresAt: '2026-11-26T09:00:00Z',
    }, 202);
    expect(run.request.body).toEqual({ selection: { query: 'status = todo' }, action: { kind: 'transition', statusKey: 'done' } });
    // Running in the background: said so, with where to follow it; the search run again.
    await searched({ items: first, nextCursor: null, total: 120 });
    expect(text(page().querySelector('.bulk-running'))).toBe('120 tickets are being changed in the background: you will be notified when it is done. Follow it');
    expect(page().querySelector('.bulk-running a')?.getAttribute('href')).toBe('/bulk-actions/b1');
    // The search run again starts with nothing selected; the selection can also be cleared.
    expect(page().querySelector('.bulk-bar')).toBeNull();
    await click(page().querySelector('td.select mat-checkbox input'));
    expect(text(page().querySelector('.bulk-count'))).toBe('1 selected');
    await click(page().querySelector('button.bulk-clear'));
    expect(page().querySelector('.bulk-bar')).toBeNull();
    expect(texts('td.key a')).toEqual(['PLAT-1', 'PLAT-2', 'PLAT-3']);
  });

  it('says when nothing matches, and when the search failed', async () => {
    const { searched, page, text, click } = await open('/search?q=key%20%3D%20NOPE-1');
    await searched({ items: [], nextCursor: null, total: 0 });
    expect(text(page().querySelector('ui-empty-state'))).toContain('No ticket matches this query');
    expect(await axeViolations(page())).toEqual([]);

    await click(page().querySelector('button.run'));
    await searched({ status: 500, code: 'internal', title: 'Internal error' }, 500);
    expect(page().querySelector('ui-error-state')).not.toBeNull();
    await click(page().querySelector('ui-error-state button'));
    await searched({ items: [result(1)], nextCursor: null, total: 1 });
    expect(page().querySelector('ui-error-state')).toBeNull();
  });

  it('opens in the builder the first time, and runs the query its criteria write', async () => {
    const { searched, parsed, page, root, click } = await open('/search', null);
    expect((await parsed({ query: '', filter: null, orderBy: [] })).request.body).toEqual({ query: '' });
    await searched({ items: [], nextCursor: null, total: 0 });
    expect(page().querySelector('app-query-builder')).not.toBeNull();
    expect(page().querySelector('app-query-editor')).toBeNull();
    const toggle = page().querySelector('mat-button-toggle.builder-mode button');
    expect([toggle?.getAttribute('role'), toggle?.getAttribute('aria-checked')]).toEqual(['radio', 'true']);

    await click(page().querySelector('mat-select.statuses'));
    await click(Array.from(root().querySelectorAll('mat-option')).find((o) => o.textContent?.includes('In progress')));
    const run = await searched({ items: [result(1)], nextCursor: null, total: 1 });
    expect(run.request.body).toEqual({ query: 'status IN (in_progress)', limit: 50 });
    expect(decodeURIComponent(TestBed.inject(Router).url)).toBe('/search?q=status IN (in_progress)');
    expect(await axeViolations(page())).toEqual([]);
  });

  it('shows the same query in the editor, and back in the builder, the mode remembered', async () => {
    const { searched, parsed, page, click, field } = await open('/search?q=status%20IN%20(todo)');
    await searched({ items: [], nextCursor: null, total: 0 });
    expect(field().value).toBe('status IN (todo)');

    await click(page().querySelector('mat-button-toggle.builder-mode button'));
    await parsed(statusTree('status IN (todo)', 'todo'));
    expect(page().querySelector('mat-select.statuses')?.textContent).toContain('To do');
    expect(localStorage.getItem('mlbpp.search.mode')).toBe('builder');

    await click(page().querySelector('mat-button-toggle.query-mode button'));
    expect(field().value).toBe('status IN (todo)');
    expect(localStorage.getItem('mlbpp.search.mode')).toBe('query');
  });

  it('keeps in the editor a query the builder cannot show, and says why', async () => {
    const query = 'status = todo OR status = done';
    const { searched, parsed, page, text, click, field } = await open(`/search?q=${encodeURIComponent(query)}`);
    await searched({ items: [], nextCursor: null, total: 0 });
    await click(page().querySelector('mat-button-toggle.builder-mode button'));
    await parsed({
      query,
      filter: {
        kind: 'or',
        operands: [
          { kind: 'clause', field: 'status', operator: '=', values: [{ kind: 'text', value: 'todo' }] },
          { kind: 'clause', field: 'status', operator: '=', values: [{ kind: 'text', value: 'done' }] },
        ],
      },
      orderBy: [],
    });
    expect(page().querySelector('app-query-builder')).toBeNull();
    expect(page().querySelector('mat-button-toggle.query-mode button')?.getAttribute('aria-checked'), 'the toggle goes back').toBe('true');
    expect(field().value).toBe(query);
    expect(text(page().querySelector('.notice[role="status"]'))).toBe(
      'This query cannot be shown in the builder: it stays in the editor.',
    );
    expect(localStorage.getItem('mlbpp.search.mode')).toBe('query');
  });

  it('opens a saved filter from the address: its query runs, its name and sharing above the results', async () => {
    const { searched, replied, settle, page, text, field } = await open('/search?filter=f1', 'query', [LATE]);
    const read = await replied((url) => url === '/api/v1/saved-filters/f1', LATE);
    expect(read.request.method).toBe('GET');
    const ran = await searched({ items: [result(1), result(2)], nextCursor: null, total: 2 });
    expect(ran.request.body).toEqual({ query: 'due < now()', limit: 50 });
    expect(field().value).toBe('due < now()');
    expect(text(page().querySelector('.subtitle'))).toBe('Filter “Late” · shared · Results: 2');
    expect(page().querySelector('app-saved-filters a.link')?.getAttribute('aria-current')).toBe('page');
    expect(await axeViolations(page())).toEqual([]);

    // Changed: still the filter, said so; the address keeps both.
    field().value = 'due < now() AND status != done';
    field().dispatchEvent(new Event('input'));
    field().dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }));
    await settle();
    await searched({ items: [result(1)], nextCursor: null, total: 1 });
    expect(text(page().querySelector('.subtitle'))).toBe('Filter “Late” · shared · query changed · Results: 1');
    expect(decodeURIComponent(TestBed.inject(Router).url)).toBe('/search?filter=f1&q=due < now() AND status != done');
  });

  it('saves the query run as a filter, then opens it', async () => {
    const { searched, replied, page, text, click } = await open('/search?q=type%20%3D%20bug');
    await searched({ items: [result(1)], nextCursor: null, total: 1 });
    await click(page().querySelector('button.save-filter'));
    const dialog = document.querySelector('mat-dialog-container')!;
    expect(text(dialog.querySelector('.query'))).toBe('type = bug');
    const name = dialog.querySelector('input[formControlName="name"]') as HTMLInputElement;
    name.value = 'Bugs';
    name.dispatchEvent(new Event('input'));
    await click(dialog.querySelector('button.save'));
    const saved = await replied((url, method) => url === '/api/v1/saved-filters' && method === 'POST', { ...LATE, id: 'f9', name: 'Bugs', query: 'type = bug', sharing: { all: false, groups: [] } }, 201);
    expect((saved.request.body as { query: string }).query).toBe('type = bug');
    await replied((url) => url === '/api/v1/saved-filters/f9', { ...LATE, id: 'f9', name: 'Bugs', query: 'type = bug', sharing: { all: false, groups: [] } });
    await searched({ items: [result(1)], nextCursor: null, total: 1 });
    expect(decodeURIComponent(TestBed.inject(Router).url)).toBe('/search?filter=f9');
    expect(text(page().querySelector('.notice'))).toBe('Filter saved.');
    expect(text(page().querySelector('.subtitle'))).toBe('Filter “Bugs” · personal · Results: 1');
  });

  it('says when the filter of the address can no longer be read, and searches everything', async () => {
    const { searched, replied, page, text } = await open('/search?filter=gone');
    await replied(
      (url) => url === '/api/v1/saved-filters/gone',
      { type: 'urn:mlbpp:problem:not_found', title: 'Not found', status: 404, code: 'not_found' },
      404,
    );
    const ran = await searched({ items: [], nextCursor: null, total: 0 });
    expect(ran.request.body).toEqual({ query: '', limit: 50 });
    expect(text(page().querySelector('.filter-error'))).toBe('This filter no longer exists, or is no longer shared with you.');
  });
});
