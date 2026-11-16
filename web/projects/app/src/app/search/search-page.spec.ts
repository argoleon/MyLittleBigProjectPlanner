/**
 * Tests of the search screen (L06-US01), the API mocked at the HTTP level
 * and the real translations: the query of the address run and its results
 * with their total, Enter running a query, a refused query shown at its
 * position, sorting a column sorting the query, the next page loaded
 * through the cursor, the columns of the member's choice, empty and failed
 * searches, and accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { Router, provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { SearchResult, provideApi, provideMaterialDefaults } from 'ui';
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

describe('the search screen', () => {
  async function open(url: string) {
    localStorage.clear();
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
    const settle = async () => {
      for (let i = 0; i < 4; i++) {
        await new Promise((resolve) => setTimeout(resolve, 20));
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
    return { http, harness, settle, searched, root, page, text, texts, field, click };
  }

  it('runs the query of the address and shows the results with their total', async () => {
    const { searched, page, text, texts, field } = await open('/search?q=status%20%3D%20todo');
    const request = await searched({ items: [result(1), result(2, { priority: 'low', assignee: null })], nextCursor: null, total: 2 });
    expect(request.request.body).toEqual({ query: 'status = todo', limit: 50 });
    expect(field().value).toBe('status = todo');
    expect(text(page().querySelector('.subtitle'))).toBe('Results: 2');
    expect(texts('th')).toEqual(['Key', 'Summary', 'Type', 'Status', 'Assignee', 'Priority', 'Due']);
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
    expect(texts('th')).toEqual(['Key', 'Summary', 'Status', 'Assignee', 'Reporter', 'Priority', 'Due']);
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
});
