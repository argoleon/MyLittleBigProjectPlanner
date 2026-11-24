/**
 * Tests of the capacity of the next 4 weeks (L07-US06): the weekly totals
 * (unit), and the card with the API mocked at the HTTP level and the real
 * translations (integration): the request from this week's Monday, the
 * weeks and their hours, loading again, errors, accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { CapacityPage, addDays, isoWeek, mondayOf, provideApi, provideMaterialDefaults, toLocalDate } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { CapacityWeeks, WEEKS, weekTotals } from './capacity-weeks';

const RESOURCE = '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b01';

describe('weekTotals', () => {
  it('adds the minutes of each week from its Monday', () => {
    const minutes = [420, 420, 420, 420, 0, 0, 0, 420, 420, 0, 420, 210, 0, 0];
    expect(weekTotals('2026-12-21', minutes)).toEqual([
      { week: 52, from: '2026-12-21', to: '2026-12-27', minutes: 1680 },
      { week: 53, from: '2026-12-28', to: '2027-01-03', minutes: 1470 },
    ]);
    expect(weekTotals('2026-12-21', [])).toEqual([]);
  });
});

describe('CapacityWeeks', () => {
  const monday = mondayOf(toLocalDate(new Date()));
  const sunday = addDays(monday, WEEKS * 7 - 1);
  const url = `/api/v1/capacity?resources=${RESOURCE}&from=${monday}&to=${sunday}`;

  function page(minutes: number[]): CapacityPage {
    return {
      from: monday,
      to: sunday,
      items: [{ resourceId: RESOURCE, displayName: 'Camille Martin', active: true, minutes, totalMinutes: 0, unavailable: [] }],
      nextCursor: null,
    };
  }

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
    const reply = async (body: object, status = 200): Promise<TestRequest> => {
      let request: TestRequest | undefined;
      for (let i = 0; i < 50 && !request; i++) {
        [request] = http.match((r) => r.urlWithParams === url);
        if (!request) await new Promise((resolve) => setTimeout(resolve, 20));
      }
      if (!request) throw new Error(`no request to ${url} among ${http.match(() => true).map((r) => r.request.urlWithParams).join(', ') || 'none'}`);
      request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error' });
      await settle();
      return request;
    };
    const fixture = TestBed.createComponent(CapacityWeeks);
    fixture.componentRef.setInput('resourceId', RESOURCE);
    const host = fixture.nativeElement as HTMLElement;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const rows = () => Array.from(host.querySelectorAll('.week')).map((row) => text(row));
    return { fixture, host, settle, reply, text, rows };
  }

  afterEach(() => TestBed.inject(HttpTestingController).verify());

  it('shows the hours of the next 4 weeks from this week’s Monday', async () => {
    const { fixture, host, settle, reply, text, rows } = setup();
    await settle();
    expect(host.querySelector('mat-progress-bar')?.getAttribute('aria-label')).toBe('Loading the capacity of the weeks…');
    const week = [420, 420, 420, 420, 420, 0, 0];
    await reply(page([...week, 420, 420, 0, 420, 210, 0, 0, ...week, 0, 0, 0, 0, 0, 0, 0]));
    expect(text(host.querySelector('h2'))).toBe('Capacity over the next 4 weeks');
    expect(rows().length).toBe(4);
    const first = host.querySelector('.week');
    expect(first?.getAttribute('data-week')).toBe(monday);
    expect(text(first?.querySelector('abbr'))).toBe(`W${isoWeek(monday)}`);
    expect(first?.querySelector('abbr')?.getAttribute('title')).toBe(`Week ${isoWeek(monday)}`);
    expect(Array.from(host.querySelectorAll('.hours')).map((hours) => text(hours))).toEqual(['35 h', '24 h 30 min', '35 h', '0 h']);
    expect(text(first?.querySelector('.range'))).toMatch(/^[A-Z][a-z]{2} \d{1,2} to [A-Z][a-z]{2} \d{1,2}$/);
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
  });

  it('reads the weeks again when asked, and tells an error', async () => {
    const { fixture, host, settle, reply, rows } = setup();
    await settle();
    await reply(page(Array<number>(28).fill(420)));
    expect(rows().length).toBe(4);
    fixture.componentInstance.reload();
    await settle();
    await reply({ type: 'urn:mlbpp:problem:internal', title: 'internal', status: 500, code: 'internal', errors: [] }, 500);
    expect(host.querySelector('ui-error-state')).not.toBeNull();
  });
});
