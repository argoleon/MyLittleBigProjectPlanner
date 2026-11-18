/**
 * Tests of the dialog saving a filter (L06-US04), the API mocked at the HTTP
 * level and the real translations: a new private filter, one shared with
 * groups found by name, a name already taken, a filter of the member changed
 * or saved as a new one, and accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { SavedFilter, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { SaveFilterData, SaveFilterDialog, sharingOf } from './save-filter-dialog';

const MINE: SavedFilter = {
  id: 'f1',
  name: 'Late',
  icon: 'schedule',
  query: 'due < now()',
  owner: { accountId: 'a1', displayName: 'Lucas Bernard', member: true },
  sharing: { all: false, groups: [{ id: 'g1', name: 'Platform' }] },
  mine: true,
  canEdit: true,
  canDelete: true,
  version: 3,
};

describe('the dialog saving a filter', () => {
  async function open(data: SaveFilterData) {
    TestBed.configureTestingModule({
      providers: [provideApi(), provideHttpClientTesting(), provideI18nTesting(), provideMaterialDefaults()],
    });
    const http = TestBed.inject(HttpTestingController);
    const ref = TestBed.inject(MatDialog).open(SaveFilterDialog, { data });
    let closed: SavedFilter | undefined | null = null;
    // As soon as it closes, before its animation.
    ref.beforeClosed().subscribe((saved) => (closed = saved));
    const settle = async () => {
      for (let i = 0; i < 3; i++) {
        await new Promise((resolve) => setTimeout(resolve, 20));
        TestBed.tick();
      }
    };
    await settle();
    const dialog = () => document.querySelector('mat-dialog-container')!;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const click = async (element: Element | null | undefined) => {
      (element as HTMLElement).click();
      await settle();
    };
    const type = async (input: Element | null, value: string) => {
      (input as HTMLInputElement).focus();
      input!.dispatchEvent(new Event('focusin'));
      (input as HTMLInputElement).value = value;
      input!.dispatchEvent(new Event('input'));
      await settle();
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
    const radio = (label: string) =>
      Array.from(dialog().querySelectorAll('mat-radio-button')).find((r) => text(r) === label)?.querySelector('input');
    return { http, dialog, text, click, type, reply, radio, closed: () => closed };
  }

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it('saves the query as a private filter, with the icon chosen', async () => {
    const { dialog, text, click, type, reply, closed } = await open({ query: 'type = bug' });
    expect(text(dialog().querySelector('h2'))).toBe('Save the filter');
    expect(text(dialog().querySelector('.query'))).toBe('type = bug');
    expect(dialog().querySelector('.save-new')).toBeNull();
    await type(dialog().querySelector('input[formControlName="name"]'), '  Bugs  ');
    await click(dialog().querySelector('mat-button-toggle button[aria-label="Bug"]'));
    expect(await axeViolations(dialog())).toEqual([]);
    await click(dialog().querySelector('button.save'));
    const saved = await reply((url, method) => url === '/api/v1/saved-filters' && method === 'POST', { ...MINE, name: 'Bugs' }, 201);
    expect(saved.request.headers.get('Idempotency-Key')).toBeTruthy();
    expect(saved.request.body).toEqual({
      name: 'Bugs',
      icon: 'bug_report',
      query: 'type = bug',
      sharing: { all: false, groupIds: [] },
    });
    expect(closed()?.name).toBe('Bugs');
  });

  it('shares with groups found by name, at least one', async () => {
    const { dialog, text, click, type, reply, radio } = await open({ query: 'type = bug' });
    await type(dialog().querySelector('input[formControlName="name"]'), 'Bugs');
    await click(radio('Groups'));
    await reply((url) => url === '/api/v1/saved-filters/share-groups?limit=10', [{ id: 'g1', name: 'Platform' }]);
    // None chosen yet: said so, nothing sent.
    await click(dialog().querySelector('button.save'));
    expect(text(dialog().querySelector('.groups-error'))).toBe('Choose at least one group.');

    await type(dialog().querySelector('input.group-search'), 'pay');
    await reply((url) => url === '/api/v1/saved-filters/share-groups?search=pay&limit=10', [
      { id: 'g2', name: 'Payroll' },
      { id: 'g3', name: 'Paying customers' },
    ]);
    const options = Array.from(document.querySelectorAll('mat-option'));
    expect(options.map((o) => text(o))).toEqual(['Payroll', 'Paying customers']);
    await click(options[0]);
    expect(Array.from(dialog().querySelectorAll('mat-chip-row')).map((c) => text(c))).toEqual(['Payroll cancel']);
    expect(dialog().querySelector('.groups-error')).toBeNull();
    expect(await axeViolations(dialog())).toEqual([]);
    await click(dialog().querySelector('button.save'));
    const saved = await reply((url, method) => url === '/api/v1/saved-filters' && method === 'POST', MINE, 201);
    expect((saved.request.body as { sharing: object }).sharing).toEqual({ all: false, groupIds: ['g2'] });
  });

  it('shows a name already taken next to the name', async () => {
    const { dialog, text, click, type, reply, radio } = await open({ query: 'type = bug' });
    await type(dialog().querySelector('input[formControlName="name"]'), 'Bugs');
    await click(radio('The whole organization'));
    await click(dialog().querySelector('button.save'));
    const saved = await reply(
      (url, method) => url === '/api/v1/saved-filters' && method === 'POST',
      {
        type: 'urn:mlbpp:problem:saved_filter.name_taken',
        title: 'taken',
        status: 409,
        code: 'saved_filter.name_taken',
        errors: [{ field: 'name', code: 'taken' }],
      },
      409,
    );
    expect((saved.request.body as { sharing: object }).sharing).toEqual({ all: true, groupIds: [] });
    expect(text(dialog().querySelector('mat-error'))).toBe('This value is already used.');
  });

  it('changes a filter of the member from its version, or saves it as a new one', async () => {
    const { dialog, text, click, type, reply, closed } = await open({ query: 'due < now() AND status != done', filter: MINE });
    expect(text(dialog().querySelector('h2'))).toBe('Change the filter');
    expect((dialog().querySelector('input[formControlName="name"]') as HTMLInputElement).value).toBe('Late');
    expect(Array.from(dialog().querySelectorAll('mat-chip-row')).map((c) => text(c))).toEqual(['Platform cancel']);
    await click(dialog().querySelector('button.save'));
    await reply((url) => url.startsWith('/api/v1/saved-filters/share-groups'), []);
    const changed = await reply((url, method) => url === '/api/v1/saved-filters/f1' && method === 'PUT', { ...MINE, version: 4 });
    expect(changed.request.headers.get('If-Match')).toBe('"3"');
    expect(changed.request.body).toEqual({
      name: 'Late',
      icon: 'schedule',
      query: 'due < now() AND status != done',
      sharing: { all: false, groupIds: ['g1'] },
    });
    expect(closed()?.version).toBe(4);

    TestBed.inject(MatDialog).closeAll();
    TestBed.resetTestingModule();
    const again = await open({ query: 'due < now()', filter: MINE });
    await type(again.dialog().querySelector('input[formControlName="name"]'), 'Late, a copy');
    await again.click(again.dialog().querySelector('button.save-new'));
    await again.reply((url) => url.startsWith('/api/v1/saved-filters/share-groups'), []);
    const created = await again.reply((url, method) => url === '/api/v1/saved-filters' && method === 'POST', MINE, 201);
    expect((created.request.body as { name: string }).name).toBe('Late, a copy');
  });

  it('reads the sharing of a filter', () => {
    expect(sharingOf(undefined)).toBe('private');
    expect(sharingOf(MINE)).toBe('groups');
    expect(sharingOf({ ...MINE, sharing: { all: true, groups: [] } })).toBe('all');
    expect(sharingOf({ ...MINE, sharing: { all: false, groups: [] } })).toBe('private');
  });
});
