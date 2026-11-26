/**
 * Tests of the heatmap screen (L08-US06), the API mocked at the HTTP level
 * and the real translations: the range and filters it asks for (scale,
 * period, team, node, provisional load), the summary, the drawn heatmap
 * and the equivalent table, a cell opened in the side panel with its
 * tickets, busy time and forced overload, accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { By } from '@angular/platform-browser';
import { provideRouter } from '@angular/router';
import { Heatmap as HeatmapData, HeatmapCell, HeatmapGrid, provideApi, provideMaterialDefaults, toLocalDate } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { anchorOf } from './heatmap-model';
import { HeatmapPage } from './heatmap-page';

const DATA: HeatmapData = {
  scale: 'week',
  periods: [
    { start: '2026-11-16', end: '2026-11-22' },
    { start: '2026-11-23', end: '2026-11-29' },
  ],
  rows: [
    {
      resource: { id: 'r1', kind: 'person', displayName: 'Camille Martin', jobTitle: 'Dev back', linked: true },
      load: [2352, 1050],
      provisional: [0, 0],
      hidden: [0, 600],
      capacity: [2100, 2100],
      scheduled: [2100, 2100],
    },
    {
      resource: { id: 'r2', kind: 'generic', displayName: 'Rust developer', linked: false },
      load: [840, 0],
      provisional: [840, 0],
      hidden: [0, 0],
      capacity: [2100, 0],
      scheduled: [2100, 2100],
    },
  ],
  tolerancePercent: 0,
  includeProvisional: true,
};

const CELL: HeatmapCell = {
  resource: { id: 'r1', kind: 'person', displayName: 'Camille Martin', linked: true },
  from: '2026-11-16',
  to: '2026-11-22',
  capacityMinutes: 2100,
  scheduledMinutes: 2100,
  loadMinutes: 2352,
  provisionalMinutes: 0,
  items: [
    {
      issue: { id: 'i1', key: 'PLAT-142', summary: 'OIDC sign-in', project: { id: 'p1', key: 'PLAT', name: 'Identity platform' } },
      minutes: 1050,
      provisional: false,
      segments: [{ mode: 'percentage', value: 50 }],
      inNode: true,
    },
    { issue: null, minutes: 1302, provisional: false, segments: [], inNode: false },
  ],
  forced: [{ issueKey: 'PLAT-142', actorName: 'Lucas Bernard', createdAt: '2026-11-03T09:00:00Z' }],
};

describe('HeatmapPage', () => {
  function setup() {
    localStorage.clear();
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
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    return { http, settle, reply, text };
  }

  const isHeatmap = (url: string) => url.startsWith('/api/v1/workload/heatmap?');

  async function open() {
    const kit = setup();
    const fixture = TestBed.createComponent(HeatmapPage);
    await kit.settle();
    await kit.reply((url) => url === '/api/v1/teams', {
      items: [{ id: 't1', name: 'Platform', nameTranslations: { en: 'Platform' }, memberCount: 2, version: 1 }],
      canManage: false,
    });
    await kit.reply((url) => url.startsWith('/api/v1/nodes?'), {
      items: [
        { id: 'n1', type: 'program', name: 'Modernization', key: 'MOD', parents: [] },
        { id: 'n2', type: 'project', parents: [], veiled: true },
      ],
      nextCursor: null,
    });
    const first = await kit.reply(isHeatmap, DATA);
    const host = fixture.nativeElement as HTMLElement;
    const grid = () => fixture.debugElement.query(By.directive(HeatmapGrid))?.componentInstance as HeatmapGrid | undefined;
    return { ...kit, fixture, host, first, grid };
  }

  afterEach(() => {
    localStorage.clear();
    TestBed.inject(HttpTestingController).verify();
  });

  it('asks for eight weeks from this Monday, and draws the resources with their averages', async () => {
    const { first, host, text, grid, fixture } = await open();
    const monday = anchorOf('week', toLocalDate(new Date()));
    expect(first.request.params.get('scale')).toBe('week');
    expect(first.request.params.get('from')).toBe(monday);
    expect(first.request.params.get('includeProvisional')).toBe('true');
    expect(first.request.params.has('teamId')).toBe(false);
    expect(text(host.querySelector('.summary'))).toMatch(/^Resources: 2 · from .* to .* · Overloads: 1$/);
    const heatmap = grid();
    expect(heatmap?.rows().map((r) => r.label)).toEqual(['Camille Martin', 'Rust developer', 'Average']);
    expect(heatmap?.columns().map((c) => c.label)).toEqual(['W47', 'W48']);
    expect(heatmap?.cells()[0][0]).toMatchObject({ level: 'over', text: '112%' });
    expect(heatmap?.label()).toBe('Resource workload');
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
  });

  it('asks again for another scale, period, team, node or without provisional load', async () => {
    const { host, reply, settle, text, fixture } = await open();
    const toggle = (label: string) =>
      Array.from(host.querySelectorAll<HTMLButtonElement>('mat-button-toggle button')).find((b) => text(b) === label) ?? null;
    toggle('Day')?.click();
    await settle();
    const days = await reply(isHeatmap, { ...DATA, scale: 'day' });
    expect(days.request.params.get('scale')).toBe('day');
    const from = days.request.params.get('from') ?? '';
    host.querySelector<HTMLButtonElement>('button.next')?.click();
    await settle();
    const next = await reply(isHeatmap, { ...DATA, scale: 'day' });
    expect((next.request.params.get('from') ?? '') > from).toBe(true);
    // The team and the program; a veiled node is not offered.
    const page = fixture.componentInstance as unknown as { setTeam(id: string | null): void; setNode(id: string | null): void };
    page.setTeam('t1');
    await settle();
    expect((await reply(isHeatmap, DATA)).request.params.get('teamId')).toBe('t1');
    page.setNode('n1');
    await settle();
    expect((await reply(isHeatmap, DATA)).request.params.get('nodeId')).toBe('n1');
    host.querySelector<HTMLElement>('mat-chip-option .mdc-evolution-chip__action')?.click();
    await settle();
    expect((await reply(isHeatmap, DATA)).request.params.get('includeProvisional')).toBe('false');
  });

  it('opens a cell in the side panel with its tickets, busy time and forced overload', async () => {
    const { host, reply, text, grid, settle, fixture } = await open();
    grid()?.activate.emit({ row: 0, column: 0 });
    await settle();
    const asked = await reply((url) => url.startsWith('/api/v1/workload/cell?'), CELL);
    expect(asked.request.params.get('resourceId')).toBe('r1');
    expect(asked.request.params.get('from')).toBe('2026-11-16');
    expect(asked.request.params.get('to')).toBe('2026-11-22');
    const panel = host.querySelector('aside.panel');
    expect(text(panel?.querySelector('h2'))).toBe('Camille Martin');
    expect(text(panel?.querySelector('.eyebrow'))).toBe('W47 · from Nov 16 to Nov 22');
    expect(Array.from(panel?.querySelectorAll('.tile dd') ?? []).map((dd) => text(dd))).toEqual(['35 h', '39 h 12 min', '+4 h 12 min']);
    expect(panel?.querySelector('.tile.over')).not.toBeNull();
    expect(text(panel?.querySelector('.message.forced'))).toMatch(/^warning Overload forced on PLAT-142 by Lucas Bernard on Nov 3, 2026\.$/);
    const items = Array.from(panel?.querySelectorAll('li.item') ?? []);
    expect(text(items[0].querySelector('.issue-key'))).toBe('PLAT-142');
    expect(Array.from(items[0].querySelectorAll('.tag')).map((t) => text(t))).toEqual(['50% of the capacity', 'In the filter']);
    // AC5: busy time, without name.
    expect(text(items[1].querySelector('.busy'))).toBe('Busy (confidential)');
    expect(items[1].querySelector('a')).toBeNull();
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
    // The averages have no details; closing hides the panel.
    grid()?.activate.emit({ row: 2, column: 0 });
    await settle();
    expect(host.querySelector('aside.panel')).toBeNull();
  });

  it('lists the same data in an accessible table, each cell opening the panel', async () => {
    const { host, reply, text, settle, fixture } = await open();
    Array.from(host.querySelectorAll<HTMLButtonElement>('mat-button-toggle button')).find((b) => text(b) === 'Table')?.click();
    await settle();
    expect(host.querySelector('ui-heatmap')).toBeNull();
    const rows = Array.from(host.querySelectorAll('tr[mat-row]'));
    expect(rows.map((r) => text(r.querySelector('th .name')))).toEqual(['Camille Martin', 'Rust developer', 'Average']);
    const first = rows[0].querySelector<HTMLButtonElement>('button.cell-button');
    expect(text(first?.querySelector('.cdk-visually-hidden'))).toBe('112%, 39 h 12 min planned for 35 h available, overloaded');
    expect(rows[2].querySelector('button')).toBeNull();
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
    first?.click();
    await settle();
    await reply((url) => url.startsWith('/api/v1/workload/cell?'), CELL);
    expect(text(host.querySelector('aside.panel h2'))).toBe('Camille Martin');
    host.querySelector<HTMLButtonElement>('aside.panel button.close')?.click();
    await settle();
    expect(host.querySelector('aside.panel')).toBeNull();
  });
});
