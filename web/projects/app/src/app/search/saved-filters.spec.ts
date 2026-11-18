/**
 * Tests of the saved filters panel (L06-US04), the API mocked at the HTTP
 * level and the real translations: the member's filters and those shared
 * with them, the one open, the menu of each as the member may use it, a
 * deletion after confirmation, a failed load, and accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter } from '@angular/router';
import { SavedFilter, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { SavedFilters } from './saved-filters';

function filter(id: string, name: string, overrides: Partial<SavedFilter> = {}): SavedFilter {
  return {
    id,
    name,
    icon: 'filter_alt',
    query: 'type = bug',
    owner: { accountId: 'a1', displayName: 'Lucas Bernard', member: true },
    sharing: { all: false, groups: [] },
    mine: true,
    canEdit: true,
    canDelete: true,
    version: 1,
    ...overrides,
  };
}

const MINE = filter('f1', 'My open tickets', { icon: 'person' });
const MINE_SHARED = filter('f2', 'Critical bugs', { icon: 'bug_report', sharing: { all: true, groups: [] } });
const OTHERS = filter('f3', 'Late', {
  icon: 'schedule',
  mine: false,
  canEdit: false,
  canDelete: false,
  owner: { accountId: 'a2', displayName: 'Camille Martin', member: true },
  sharing: { all: false, groups: [{ id: 'g1', name: 'Platform' }] },
});

describe('the saved filters panel', () => {
  async function open(items: readonly SavedFilter[] | null, active: string | null = null) {
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
      request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error' });
      await settle();
      return request;
    };
    const fixture = TestBed.createComponent(SavedFilters);
    fixture.componentRef.setInput('active', active);
    const edited: SavedFilter[] = [];
    const deleted: SavedFilter[] = [];
    fixture.componentInstance.edit.subscribe((f) => edited.push(f));
    fixture.componentInstance.deleted.subscribe((f) => deleted.push(f));
    document.body.appendChild(fixture.nativeElement);
    await settle();
    const list = () => (items ? reply((url, method) => url === '/api/v1/saved-filters' && method === 'GET', { items }) : null);
    await list();
    const panel = () => fixture.nativeElement as HTMLElement;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const texts = (selector: string, within: ParentNode = panel()) => Array.from(within.querySelectorAll(selector)).map((e) => text(e));
    const click = async (element: Element | null | undefined) => {
      (element as HTMLElement).click();
      await settle();
    };
    return { http, reply, panel, text, texts, click, edited, deleted };
  }

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it('lists the member’s filters, then those shared with them, each opening the search', async () => {
    const { panel, texts } = await open([MINE_SHARED, OTHERS, MINE], 'f2');
    expect(texts('h2.section')).toEqual(['My filters', 'Shared with me']);
    const [mine, shared] = Array.from(panel().querySelectorAll('ul.items'));
    expect(texts('.name', mine)).toEqual(['Critical bugs', 'My open tickets']);
    expect(texts('.name', shared)).toEqual(['Late']);
    // The member's shared one is marked; the others' say whose.
    // What a screen reader reads of a link: its text without what it hides.
    const link = (within: Element) => {
      const copy = within.querySelector('a.link')!.cloneNode(true) as Element;
      copy.querySelectorAll('[aria-hidden="true"]').forEach((hidden) => hidden.remove());
      return copy.textContent!.replace(/\s+/g, ' ').trim();
    };
    expect(link(mine)).toBe('Critical bugs (shared)');
    expect(link(shared)).toBe('Late (by Camille Martin)');
    const links = Array.from(panel().querySelectorAll('a.link'));
    expect(links.map((a) => a.getAttribute('href'))).toEqual(['/search?filter=f2', '/search?filter=f1', '/search?filter=f3']);
    expect(links.map((a) => a.getAttribute('aria-current'))).toEqual(['page', null, null]);
    expect(texts('.item.active .name')).toEqual(['Critical bugs']);
    // Nothing to do on another member's filter.
    expect(shared.querySelector('button.actions')).toBeNull();
    expect(await axeViolations(panel())).toEqual([]);
  });

  it('says when there is no filter yet', async () => {
    const { texts } = await open([]);
    expect(texts('p.empty')).toEqual(['Save a search to find it here.', 'No filter is shared with you.']);
  });

  it('asks for a change of the member’s filter, and deletes one once confirmed', async () => {
    const { panel, text, click, reply, edited, deleted } = await open([MINE]);
    await click(panel().querySelector('button.actions'));
    expect(panel().querySelector('button.actions')?.getAttribute('aria-label')).toBe('Actions of the filter My open tickets');
    await click(document.querySelector('.mat-mdc-menu-panel button.edit'));
    expect(edited.map((f) => f.id)).toEqual(['f1']);

    await click(panel().querySelector('button.actions'));
    await click(document.querySelector('.mat-mdc-menu-panel button.delete'));
    const confirm = document.querySelector('mat-dialog-container')!;
    expect(text(confirm.querySelector('h2'))).toBe('Delete the filter “My open tickets”?');
    expect(text(confirm.querySelector('p'))).toBe('It will be gone from your filters.');
    await click(Array.from(confirm.querySelectorAll('button')).find((b) => text(b) === 'Delete'));
    const removed = await reply((url, method) => url === '/api/v1/saved-filters/f1' && method === 'DELETE', null, 204);
    expect(removed.request.headers.get('If-Match')).toBe('"1"');
    await reply((url, method) => url === '/api/v1/saved-filters' && method === 'GET', { items: [] });
    expect(deleted.map((f) => f.id)).toEqual(['f1']);
  });

  it('lets a tenant admin delete a shared filter of another member, and says why it failed', async () => {
    const moderated = { ...OTHERS, canDelete: true };
    const { panel, text, texts, click, reply } = await open([moderated]);
    await click(panel().querySelector('button.actions'));
    expect(document.querySelector('.mat-mdc-menu-panel button.edit')).toBeNull();
    await click(document.querySelector('.mat-mdc-menu-panel button.delete'));
    const confirm = document.querySelector('mat-dialog-container')!;
    expect(text(confirm.querySelector('p'))).toBe('It will be gone for everyone it is shared with.');
    await click(Array.from(confirm.querySelectorAll('button')).find((b) => text(b) === 'Delete'));
    await reply(
      (url, method) => url === '/api/v1/saved-filters/f3' && method === 'DELETE',
      { type: 'urn:mlbpp:problem:precondition_failed', title: 'stale', status: 412, code: 'precondition_failed' },
      412,
    );
    await reply((url, method) => url === '/api/v1/saved-filters' && method === 'GET', { items: [moderated] });
    expect(texts('[role="alert"]')).toHaveLength(1);
  });

  it('offers to load the filters again after a failure', async () => {
    const { texts, panel, click, reply } = await open(null);
    await reply(
      (url) => url === '/api/v1/saved-filters',
      { type: 'urn:mlbpp:problem:internal', title: 'boom', status: 500, code: 'internal' },
      500,
    );
    expect(texts('[role="alert"]')[0]).toContain('The filters could not be loaded.');
    await click(panel().querySelector('button.retry'));
    await reply((url) => url === '/api/v1/saved-filters', { items: [MINE] });
    expect(texts('.name')).toEqual(['My open tickets']);
  });
});
