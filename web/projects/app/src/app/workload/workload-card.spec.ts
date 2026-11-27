/**
 * Tests of the load and capacity card (L08-US07): its ranges and lines
 * (unit), and the card for a node and for a team with the API mocked at
 * the HTTP level and the real translations (integration): six months by
 * default, weeks on demand, moving by a screen, an overloaded month said
 * in words, confidential projects counted, accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { WorkloadAggregate, provideApi, provideMaterialDefaults, toLocalDate } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { anchorOf } from './heatmap-model';
import { WorkloadCard, aggregateRange, linesOf, moveAnchor } from './workload-card';

const NODE: WorkloadAggregate = {
  scale: 'month',
  periods: [
    { start: '2026-11-01', end: '2026-11-30', loadMinutes: 18_720, provisionalMinutes: 0, capacityMinutes: 20_400, usedPercent: 92 },
    { start: '2026-12-01', end: '2026-12-31', loadMinutes: 23_520, provisionalMinutes: 1200, capacityMinutes: 21_000, usedPercent: 112 },
    { start: '2027-01-01', end: '2027-01-31', loadMinutes: 0, provisionalMinutes: 0, capacityMinutes: 0, usedPercent: null },
  ],
  count: 3,
  confidentialProjects: 1,
};

const english = (key: string, params?: Record<string, string>) =>
  key === 'workload.aggregate.line' ? `${params?.['load']} of ${params?.['capacity']} · ${params?.['percent']}` : key === 'workload.aggregate.over' ? 'overloaded' : `W${params?.['number']}`;

describe('the load and capacity card', () => {
  it('reads six months or eight weeks, and moves by as many', () => {
    expect(aggregateRange('month', '2026-11-01')).toEqual({ from: '2026-11-01', to: '2027-04-30' });
    expect(aggregateRange('week', '2026-11-23')).toEqual({ from: '2026-11-23', to: '2027-01-17' });
    expect(moveAnchor('month', '2026-11-01', 1)).toBe('2027-05-01');
    expect(moveAnchor('month', '2026-11-01', -1)).toBe('2026-05-01');
    expect(moveAnchor('week', '2026-11-23', -1)).toBe('2026-09-28');
  });

  it('tells each period, an overloaded one in words', () => {
    const lines = linesOf(NODE, 'en', english);
    expect(lines.map((l) => [l.label, l.text, l.value, l.over])).toEqual([
      ['November 2026', '312 h of 340 h · 92%', 92, false],
      ['December 2026', '392 h of 350 h · 112%', 100, true],
      ['January 2027', '0 h of 0 h · –', 0, false],
    ]);
    expect(lines[1].description).toBe('December 2026: 392 h of 350 h · 112%, overloaded');
    expect(linesOf({ ...NODE, scale: 'week', periods: [{ ...NODE.periods[0], start: '2026-11-16', end: '2026-11-22' }] }, 'en', english)[0].label).toBe('W47');
  });
});

describe('WorkloadCard', () => {
  async function open(owner: 'node' | 'team') {
    localStorage.clear();
    TestBed.configureTestingModule({
      providers: [provideRouter([]), provideApi(), provideHttpClientTesting(), provideI18nTesting(), provideMaterialDefaults()],
    });
    const http = TestBed.inject(HttpTestingController);
    const fixture = TestBed.createComponent(WorkloadCard);
    fixture.componentRef.setInput('owner', owner);
    fixture.componentRef.setInput('ownerId', 'x1');
    const settle = async () => {
      for (let i = 0; i < 3; i++) {
        await new Promise((resolve) => setTimeout(resolve, 10));
        TestBed.tick();
      }
    };
    const reply = async (match: (url: string) => boolean, body: object): Promise<TestRequest> => {
      let request: TestRequest | undefined;
      for (let i = 0; i < 50 && !request; i++) {
        [request] = http.match((r) => match(r.urlWithParams));
        if (!request) await new Promise((resolve) => setTimeout(resolve, 20));
      }
      if (!request) throw new Error(`no matching request among ${http.match(() => true).map((r) => r.request.urlWithParams).join(', ') || 'none'}`);
      request.flush(body);
      await settle();
      return request;
    };
    await settle();
    const host = fixture.nativeElement as HTMLElement;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    return { http, fixture, host, settle, reply, text };
  }

  afterEach(() => {
    localStorage.clear();
    TestBed.inject(HttpTestingController).verify();
  });

  it('shows six months of a node, an overloaded one said in words, confidential projects counted', async () => {
    const { host, reply, text, fixture } = await open('node');
    const asked = await reply((url) => url.startsWith('/api/v1/nodes/x1/workload?'), NODE);
    expect(asked.request.params.get('scale')).toBe('month');
    expect(asked.request.params.get('from')).toBe(anchorOf('month', toLocalDate(new Date())));
    const periods = Array.from(host.querySelectorAll('li.period'));
    expect(periods.map((p) => text(p.querySelector('.label')))).toEqual(['November 2026', 'December 2026', 'January 2027']);
    expect(text(periods[1].querySelector('.text'))).toBe('392 h of 350 h · 112% · overloaded');
    expect(periods[1].classList.contains('over')).toBe(true);
    expect(periods[1].querySelector('mat-progress-bar')?.getAttribute('aria-label')).toBe('December 2026: 392 h of 350 h · 112%, overloaded');
    expect(text(host.querySelector('.confidential'))).toBe('Including 1 confidential project(s), counted without detail.');
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
  });

  it('asks a team by week, and moves by a screen', async () => {
    const { host, reply, settle, text } = await open('team');
    const first = await reply((url) => url.startsWith('/api/v1/teams/x1/workload?'), { ...NODE, confidentialProjects: 0 });
    expect(host.querySelector('.confidential')).toBeNull();
    Array.from(host.querySelectorAll<HTMLButtonElement>('mat-button-toggle button')).find((b) => text(b) === 'Week')?.click();
    await settle();
    const weeks = await reply((url) => url.startsWith('/api/v1/teams/x1/workload?'), { ...NODE, scale: 'week' });
    expect(weeks.request.params.get('scale')).toBe('week');
    host.querySelector<HTMLButtonElement>('button.next')?.click();
    await settle();
    const next = await reply((url) => url.startsWith('/api/v1/teams/x1/workload?'), { ...NODE, scale: 'week' });
    expect((next.request.params.get('from') ?? '') > (weeks.request.params.get('from') ?? '')).toBe(true);
    expect(first.request.params.get('scale')).toBe('month');
  });
});
