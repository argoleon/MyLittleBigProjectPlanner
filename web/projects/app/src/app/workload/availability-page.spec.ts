/**
 * Tests of the availability search (L08-US10): the period helpers and the
 * heatmap link (unit), and the tab with the API mocked at the HTTP level
 * and the real translations (integration): next week's 3 free days asked
 * at first, the resources found most free first with their free time,
 * free days and load, free hours in a team, provisional load left out,
 * the period checked before asking, no result, accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { FormControl, FormGroup } from '@angular/forms';
import { provideRouter } from '@angular/router';
import { Availability, AvailableResource, provideApi, provideMaterialDefaults, toLocalDate } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { AvailabilityCriteria, AvailabilityPage, daysBetween, heatmapLink, nextWeek, periodError } from './availability-page';

const RUST: AvailableResource = {
  resource: { id: 'r3', kind: 'generic', displayName: 'Rust developer', linked: false },
  freeMinutes: 1560,
  freeDays: 3,
  capacityMinutes: 1680,
  loadMinutes: 120,
  workingDays: 4,
};
const NATHAN: AvailableResource = {
  resource: { id: 'r2', kind: 'person', displayName: 'Nathan Roux', jobTitle: 'Sysadmin', linked: true },
  freeMinutes: 1260,
  freeDays: 3,
  capacityMinutes: 2100,
  loadMinutes: 840,
  workingDays: 5,
};
const CRITERIA: AvailabilityCriteria = {
  from: '2026-11-23',
  to: '2026-11-29',
  teamId: null,
  need: 'days',
  amount: 3,
  includeProvisional: true,
};

describe('availability helpers', () => {
  it('take next week, Monday to Sunday', () => {
    expect(nextWeek('2026-11-27')).toEqual({ from: '2026-11-30', to: '2026-12-06' });
    expect(nextWeek('2026-11-23')).toEqual({ from: '2026-11-30', to: '2026-12-06' });
    expect(nextWeek('2026-11-29')).toEqual({ from: '2026-11-30', to: '2026-12-06' });
  });

  it('check the period as the API does', () => {
    expect(daysBetween('2026-11-23', '2026-11-29')).toBe(7);
    // Across the end of October (daylight saving time in Europe).
    expect(daysBetween('2026-10-20', '2026-11-03')).toBe(15);
    expect(periodError('2026-11-23', '2026-11-29')).toBeNull();
    expect(periodError('2026-11-29', '2026-11-23')).toBe('workload.availability.errors.beforeStart');
    expect(periodError('2026-11-01', '2027-01-31')).toBeNull();
    expect(periodError('2026-11-01', '2027-02-01')).toBe('workload.availability.errors.tooLong');
  });

  it('open a resource in the heatmap on the period, with the team and provisional load of the search', () => {
    expect(heatmapLink(RUST, CRITERIA)).toEqual({ scale: 'day', date: '2026-11-23', resourceId: 'r3' });
    expect(heatmapLink(RUST, { ...CRITERIA, teamId: 't1', includeProvisional: false })).toEqual({
      scale: 'day',
      date: '2026-11-23',
      resourceId: 'r3',
      teamId: 't1',
      provisional: 'false',
    });
  });
});

describe('AvailabilityPage', () => {
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

  const isSearch = (url: string) => url.startsWith('/api/v1/workload/availability?');
  const page = (items: AvailableResource[], criteria = CRITERIA): Availability => ({
    from: criteria.from,
    to: criteria.to,
    includeProvisional: criteria.includeProvisional,
    items,
  });

  async function open(items: AvailableResource[] = [RUST, NATHAN]) {
    const kit = setup();
    const fixture = TestBed.createComponent(AvailabilityPage);
    await kit.settle();
    await kit.reply((url) => url === '/api/v1/teams', {
      items: [{ id: 't1', name: 'Platform', nameTranslations: { en: 'Platform' }, memberCount: 3, version: 1 }],
      canManage: false,
    });
    const first = await kit.reply(isSearch, page(items));
    const host = fixture.nativeElement as HTMLElement;
    const component = fixture.componentInstance as unknown as {
      form: FormGroup<{
        start: FormControl<Date | null>;
        end: FormControl<Date | null>;
        teamId: FormControl<string | null>;
        amount: FormControl<number | null>;
      }>;
    };
    const submit = async () => {
      host.querySelector<HTMLButtonElement>('button[type="submit"]')?.click();
      await kit.settle();
    };
    return { ...kit, fixture, host, first, component, submit };
  }

  afterEach(() => {
    TestBed.inject(HttpTestingController).verify();
  });

  it('asks for next week’s 3 free days, and lists the resources found most free first', async () => {
    const { first, host, text, fixture } = await open();
    const week = nextWeek(toLocalDate(new Date()));
    expect(first.request.params.get('from')).toBe(week.from);
    expect(first.request.params.get('to')).toBe(week.to);
    expect(first.request.params.get('minDays')).toBe('3');
    expect(first.request.params.has('minHours')).toBe(false);
    expect(first.request.params.has('teamId')).toBe(false);
    expect(first.request.params.get('includeProvisional')).toBe('true');
    expect(text(host.querySelector('[role="status"]'))).toBe('2 resources available from Nov 23 to Nov 29, 2026.');
    const rows = Array.from(host.querySelectorAll('table.found tbody tr'));
    expect(rows.map((row) => Array.from(row.querySelectorAll('th, td')).map((cell) => text(cell)))).toEqual([
      ['Rust developer Generic resource', '26 h', '3', '2 h of 28 h', 'grid_view See in the heatmap'],
      ['Nathan Roux Sysadmin', '21 h', '3', '14 h of 35 h', 'grid_view See in the heatmap'],
    ]);
    const link = rows[0].querySelector('td.actions a');
    expect(link?.getAttribute('aria-label')).toBe('See in the heatmap: Rust developer');
    expect(link?.getAttribute('href')).toBe('/workload/heatmap?scale=day&date=2026-11-23&resourceId=r3');
    expect(rows[0].querySelector('th a')?.getAttribute('href')).toBe('/workload/resources/r3');
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
  });

  it('searches free hours in a team, provisional load left out', async () => {
    const { host, component, submit, reply, settle, text } = await open();
    Array.from(host.querySelectorAll<HTMLButtonElement>('mat-button-toggle button'))
      .find((b) => text(b) === 'Free hours')
      ?.click();
    await settle();
    component.form.controls.amount.setValue(22);
    component.form.controls.teamId.setValue('t1');
    component.form.controls.start.setValue(new Date(2026, 10, 16));
    component.form.controls.end.setValue(new Date(2026, 10, 22));
    host.querySelector<HTMLElement>('mat-chip-option .mdc-evolution-chip__action')?.click();
    await settle();
    await submit();
    const criteria = { ...CRITERIA, from: '2026-11-16', to: '2026-11-22', includeProvisional: false };
    const asked = await reply(isSearch, page([NATHAN], criteria));
    expect(asked.request.params.get('minHours')).toBe('22');
    expect(asked.request.params.has('minDays')).toBe(false);
    expect(asked.request.params.get('teamId')).toBe('t1');
    expect(asked.request.params.get('from')).toBe('2026-11-16');
    expect(asked.request.params.get('includeProvisional')).toBe('false');
    expect(host.querySelector('td.actions a')?.getAttribute('href')).toBe(
      '/workload/heatmap?scale=day&date=2026-11-16&resourceId=r2&teamId=t1&provisional=false',
    );
  });

  it('checks the period and the amount before asking', async () => {
    const { http, host, component, submit, text } = await open();
    component.form.controls.start.setValue(new Date(2026, 10, 1));
    component.form.controls.end.setValue(new Date(2027, 1, 1));
    await submit();
    expect(text(host.querySelector('.problem[role="alert"]'))).toBe('92 days at most.');
    component.form.controls.start.setValue(new Date(2026, 10, 16));
    component.form.controls.end.setValue(new Date(2026, 10, 22));
    component.form.controls.amount.setValue(93);
    await submit();
    expect(text(host.querySelector('mat-error'))).toBe('A whole number from 1 to 92.');
    expect(http.match(() => true).length).toBe(0);
  });

  it('says when no resource has this availability', async () => {
    const { host, text, fixture } = await open([]);
    expect(text(host.querySelector('[role="status"]'))).toBe('0 resources available from Nov 23 to Nov 29, 2026.');
    expect(text(host.querySelector('.empty'))).toContain('No resource has this availability');
    expect(host.querySelector('table')).toBeNull();
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
  });
});
