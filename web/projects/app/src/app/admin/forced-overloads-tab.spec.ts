/**
 * Tests of the "Forced overloads" tab of the administration (L08-US03),
 * the API mocked at the HTTP level and the real translations: the list,
 * newest first, with the ticket, resource, author and days; tickets,
 * resources and members gone; the next pages; the empty state;
 * accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { ForcedOverload, ForcedOverloadPage, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { ForcedOverloadsTab, PAGE_SIZE } from './forced-overloads-tab';

const FORCED: ForcedOverload = {
  id: 'f1',
  assignmentId: 'a1',
  issueId: 'i1',
  issueKey: 'PLAT-148',
  issueSummary: 'Account migration scripts',
  resourceId: 'r1',
  resourceName: 'Nathan Roux',
  actor: { id: 'm1', name: 'Léa Moreau' },
  origin: 'interface',
  days: [{ date: '2026-11-18', plannedMinutes: 480, availableMinutes: 420, limitMinutes: 420 }],
  createdAt: '2026-11-16T09:30:00Z',
};
const GONE: ForcedOverload = {
  ...FORCED,
  id: 'f2',
  issueKey: null,
  issueSummary: null,
  resourceName: null,
  actor: null,
  origin: 'api',
};

describe('ForcedOverloadsTab', () => {
  function setup() {
    TestBed.configureTestingModule({
      providers: [provideRouter([]), provideApi(), provideHttpClientTesting(), provideI18nTesting(), provideMaterialDefaults()],
    });
    const http = TestBed.inject(HttpTestingController);
    const settle = async () => {
      for (let i = 0; i < 3; i++) {
        await new Promise((resolve) => setTimeout(resolve, 10));
        TestBed.tick();
      }
    };
    const reply = async (match: (url: string) => boolean, body: object, status = 200): Promise<TestRequest> => {
      let request: TestRequest | undefined;
      for (let i = 0; i < 50 && !request; i++) {
        [request] = http.match((r) => match(r.urlWithParams));
        if (!request) await new Promise((resolve) => setTimeout(resolve, 20));
      }
      if (!request) throw new Error('no matching request');
      request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error' });
      await settle();
      return request;
    };
    const fixture = TestBed.createComponent(ForcedOverloadsTab);
    const host = fixture.nativeElement as HTMLElement;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const rows = () => Array.from(host.querySelectorAll('tr.mat-mdc-row')).map((row) => Array.from(row.querySelectorAll('td')).map((td) => text(td)));
    return { http, settle, reply, fixture, host, text, rows };
  }

  afterEach(() => TestBed.inject(HttpTestingController).verify());

  it('lists the forced overloads with their ticket, resource, author and days, page by page', async () => {
    const { reply, host, text, rows, fixture, settle } = setup();
    await settle();
    const first = await reply((url) => url.startsWith('/api/v1/tenant/forced-overloads?'), { items: [FORCED], nextCursor: 'next' } satisfies ForcedOverloadPage);
    expect(first.request.params.get('limit')).toBe(String(PAGE_SIZE));
    expect(rows().length).toBe(1);
    // The instant reads in the reader's time zone.
    expect(rows()[0][0]).toMatch(/^Nov 16, 2026, \d{1,2}:30\s(AM|PM)$/);
    expect(rows()[0].slice(1)).toEqual([
      'PLAT-148Account migration scripts',
      'Nathan Roux',
      'Léa Moreau',
      'Wed, Nov 18: 8 h planned for 7 h available (+1 h)',
    ]);
    expect(host.querySelector('a')?.getAttribute('href')).toBe('/issues/PLAT-148');
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);

    // The next page, with what is gone since.
    (Array.from(host.querySelectorAll('button')).find((b) => text(b) === 'Show more') as HTMLButtonElement).click();
    await settle();
    const second = await reply((url) => url.includes('cursor=next'), { items: [GONE], nextCursor: null } satisfies ForcedOverloadPage);
    expect(second.request.params.get('cursor')).toBe('next');
    expect(rows()[1].slice(1, 4)).toEqual(['Deleted ticket', 'Deleted resource', 'A former member via the API']);
    expect(Array.from(host.querySelectorAll('button')).some((b) => text(b) === 'Show more')).toBe(false);
  });

  it('says when nothing was forced', async () => {
    const { reply, host, text, settle } = setup();
    await settle();
    await reply((url) => url.startsWith('/api/v1/tenant/forced-overloads'), { items: [], nextCursor: null });
    expect(text(host.querySelector('ui-empty-state'))).toContain('No forced overload');
  });
});
