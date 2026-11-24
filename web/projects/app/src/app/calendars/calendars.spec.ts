/**
 * Tests of calendars (L07-US04): the helpers of the year table and the
 * month grid (unit), and the calendars tab, a calendar's page and the
 * month grid of a resource with the API mocked at the HTTP level and the
 * real translations (integration): holidays and exceptions, sites, the
 * keyboard in the grid, accessible names, rights, accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter } from '@angular/router';
import { Calendar, ResourceDay, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { CalendarPage, yearRows } from './calendar-page';
import { CalendarsPage } from './calendars-page';
import { MonthCalendar, dayLook, monthBounds, monthWeeks } from './month-calendar';

const TENANT: Calendar = {
  id: 'c0',
  kind: 'tenant',
  name: 'Organization',
  nameTranslations: { fr: 'Organisation', en: 'Organization' },
  holidaySet: 'fr',
  resourceCount: 0,
  version: 1,
};
const PARIS: Calendar = { ...TENANT, id: 'c1', kind: 'site', name: 'Paris', nameTranslations: { fr: 'Paris' }, holidaySet: null, resourceCount: 3, version: 2 };

function problem(status: number, code: string, errors: object[] = []) {
  return { type: `urn:mlbpp:problem:${code}`, title: code, status, code, errors };
}

describe('calendar helpers', () => {
  it('tell how a day looks', () => {
    const day = (status: ResourceDay['status'], minutes: number): ResourceDay => ({ day: '2026-12-25', status, minutes, source: null });
    expect(dayLook(day('holiday', 0))).toBe('holiday');
    expect(dayLook(day('non_working', 0))).toBe('closed');
    expect(dayLook(day('working', 0))).toBe('off');
    expect(dayLook(day('working', 420))).toBe('working');
  });

  it('bound a month and lay its weeks from Monday', () => {
    expect(monthBounds(2026, 11)).toEqual({ from: '2026-12-01', to: '2026-12-31' });
    expect(monthBounds(2024, 1)).toEqual({ from: '2024-02-01', to: '2024-02-29' });
    // December 2026 starts on a Tuesday.
    const weeks = monthWeeks(Array.from({ length: 31 }, (_, i) => i + 1), 1);
    expect(weeks.length).toBe(5);
    expect(weeks[0]).toEqual([null, 1, 2, 3, 4, 5, 6]);
    expect(weeks[4]).toEqual([28, 29, 30, 31, null, null, null]);
  });

  it('merge holidays and exceptions by date, the holiday first', () => {
    const rows = yearRows(
      [
        { day: '2026-05-25', name: 'whit_monday' },
        { day: '2026-01-01', name: 'new_years_day' },
      ],
      [{ id: 'e1', day: '2026-05-25', kind: 'working', version: 1 }],
    );
    expect(rows.map((r) => [r.day, r.kind])).toEqual([
      ['2026-01-01', 'set'],
      ['2026-05-25', 'set'],
      ['2026-05-25', 'working'],
    ]);
  });
});

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
  const reply = async (match: (url: string, method: string) => boolean, body: object | null, status = 200): Promise<TestRequest> => {
    let request: TestRequest | undefined;
    for (let i = 0; i < 50 && !request; i++) {
      [request] = http.match((r) => match(r.urlWithParams, r.method));
      if (!request) {
        await new Promise((resolve) => setTimeout(resolve, 20));
        TestBed.tick();
      }
    }
    if (!request) throw new Error(`no matching request among ${http.match(() => true).map((r) => r.request.urlWithParams).join(', ') || 'none'}`);
    request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error' });
    await settle();
    return request;
  };
  const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
  const click = async (element: Element | null) => {
    if (!element) throw new Error('nothing to click');
    (element as HTMLElement).click();
    await settle();
  };
  const type = async (input: Element | null, value: string) => {
    const field = input as HTMLInputElement;
    field.value = value;
    field.dispatchEvent(new Event('input'));
    field.dispatchEvent(new Event('blur'));
    await settle();
  };
  const dialog = () => Array.from(document.body.querySelectorAll('mat-dialog-container')).at(-1) ?? document.body;
  const closed = async () => {
    await new Promise((resolve) => setTimeout(resolve, 200));
    await settle();
  };
  return { http, settle, reply, text, click, type, dialog, closed };
}

describe('CalendarsPage', () => {
  async function open(items: readonly Calendar[], canManage = true) {
    const kit = setup();
    const fixture = TestBed.createComponent(CalendarsPage);
    await kit.settle();
    await kit.reply((url, method) => url === '/api/v1/calendars' && method === 'GET', { items, canManage });
    const host = fixture.nativeElement as HTMLElement;
    return { ...kit, fixture, host, rows: () => Array.from(host.querySelectorAll('tr[mat-row]')) };
  }

  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify();
  });

  it('lists the organization calendar then the sites, with their holidays', async () => {
    const { rows, text, host, fixture } = await open([TENANT, PARIS]);
    expect(
      rows().map((row) => [
        text(row.querySelector('.team-link')),
        text(row.querySelector('.secondary')),
        ...Array.from(row.querySelectorAll('td')).slice(1, 3).map((cell) => text(cell)),
      ]),
    ).toEqual([
      ['Organization', 'Organization calendar', 'France', 'Every resource without a site'],
      ['Paris', 'Site', 'Same as the organization', '3'],
    ]);
    expect(host.querySelector('a.team-link')?.getAttribute('href')).toBe('/workload/calendars/c0');
    // The organization calendar is never deleted.
    expect(host.querySelector('#calendar-delete-c0')).toBeNull();
    expect(host.querySelector('#calendar-delete-c1')).not.toBeNull();
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
  });

  it('creates a site using the Alsace-Moselle holidays', async () => {
    const { host, click, type, reply, dialog, closed, text } = await open([TENANT]);
    await click(host.querySelector('button.new'));
    expect(text(dialog().querySelector('h2'))).toBe('New site');
    await type(dialog().querySelectorAll('ui-translatable-input input')[0], 'Strasbourg');
    await click(dialog().querySelector('mat-select .mat-mdc-select-trigger'));
    await click(Array.from(document.body.querySelectorAll('mat-option')).find((o) => o.textContent?.includes('Alsace')) ?? null);
    await click(dialog().querySelector('button[type="submit"]'));
    const created = await reply((url, method) => url === '/api/v1/calendars' && method === 'POST', { ...PARIS, id: 'c2', name: 'Strasbourg' }, 201);
    expect(created.request.body).toEqual({ nameTranslations: { fr: 'Strasbourg' }, holidaySet: 'fr-alsace-moselle' });
    await closed();
    await reply((url, method) => url === '/api/v1/calendars' && method === 'GET', { items: [TENANT], canManage: true });
    expect(text(host.querySelector('[role="status"]'))).toBe('Strasbourg was created.');
  });

  it('is read-only for a member', async () => {
    const { host } = await open([TENANT, PARIS], false);
    expect(host.querySelector('button.new')).toBeNull();
    expect(host.querySelector('td button')).toBeNull();
  });
});

describe('CalendarPage', () => {
  const YEAR = new Date().getFullYear();

  async function open(canManage = true) {
    const kit = setup();
    const fixture = TestBed.createComponent(CalendarPage);
    fixture.componentRef.setInput('id', 'c1');
    await kit.settle();
    await kit.reply((url) => url === '/api/v1/calendars/c1', PARIS);
    await kit.reply((url) => url === '/api/v1/calendars', { items: [TENANT, PARIS], canManage });
    await kit.reply((url) => url === `/api/v1/calendars/c1/year?year=${YEAR}`, {
      year: YEAR,
      holidaySet: 'fr',
      inherited: true,
      holidays: [{ day: `${YEAR}-12-25`, name: 'christmas' }],
      exceptions: [{ id: 'e1', day: `${YEAR}-12-24`, kind: 'non_working', label: 'Closed', version: 1 }],
    });
    const host = fixture.nativeElement as HTMLElement;
    return { ...kit, fixture, host, rows: () => Array.from(host.querySelectorAll('tr[mat-row]')) };
  }

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it('shows the holidays and exceptions of the year, and moves to the next one', async () => {
    const { rows, text, host, click, reply, fixture } = await open();
    expect(text(host.querySelector('h1'))).toBe('Paris');
    expect(text(host.querySelector('.card .subtitle'))).toBe('Its public holidays are the organization’s.');
    expect(rows().map((row) => Array.from(row.querySelectorAll('td')).slice(1, 3).map((cell) => text(cell)))).toEqual([
      ['Closed', 'Closed'],
      ['Public holiday', 'Christmas'],
    ]);
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
    await click(host.querySelector('button.next'));
    const next = await reply((url) => url.startsWith('/api/v1/calendars/c1/year'), {
      year: YEAR + 1,
      holidaySet: 'fr',
      inherited: true,
      holidays: [],
      exceptions: [],
    });
    expect(next.request.params.get('year')).toBe(String(YEAR + 1));
  });

  it('adds an exception, and tells a day already taken', async () => {
    const { host, click, type, reply, dialog, text, closed } = await open();
    await click(host.querySelector('#exception-add'));
    expect(text(dialog().querySelector('h2'))).toBe('New exception');
    await type(dialog().querySelector('input[formcontrolname="day"]'), `12/24/${YEAR}`);
    await click(Array.from(dialog().querySelectorAll('mat-button-toggle button')).find((b) => text(b) === 'Worked') ?? null);
    expect(text(dialog().querySelector('.choice-hint'))).toContain('cancels a public holiday');
    await click(dialog().querySelector('button[type="submit"]'));
    const refused = await reply(
      (url, method) => url === '/api/v1/calendars/c1/exceptions' && method === 'POST',
      problem(409, 'calendar_exception.day_taken', [{ field: 'day', code: 'taken' }]),
      409,
    );
    expect(refused.request.body).toEqual({ day: `${YEAR}-12-24`, kind: 'working', label: null });
    expect(text(dialog().querySelector('mat-error'))).toBe('This value is already used.');
    // Closed before the page goes: the focus returns to a live button.
    TestBed.inject(MatDialog).closeAll();
    await closed();
  });
});

describe('MonthCalendar', () => {
  const DECEMBER: ResourceDay[] = Array.from({ length: 31 }, (_, i) => {
    const day = `2026-12-${String(i + 1).padStart(2, '0')}`;
    const weekday = (i + 2) % 7; // 1 December 2026 is a Tuesday (2).
    if (day === '2026-12-25') return { day, status: 'holiday', holiday: 'christmas', source: 'tenant', minutes: 0 };
    if (day === '2026-12-23') return { day, status: 'non_working', label: 'Training', source: 'resource', minutes: 0 };
    return { day, status: 'working', source: null, minutes: weekday === 0 || weekday === 6 ? 0 : 420 };
  });

  async function open(canManage = true) {
    const kit = setup();
    const fixture = TestBed.createComponent(MonthCalendar);
    fixture.componentRef.setInput('resourceId', 'r1');
    fixture.componentRef.setInput('canManage', canManage);
    (fixture.componentInstance as unknown as { month: { set(m: { year: number; month: number }): void } }).month.set({ year: 2026, month: 11 });
    await kit.settle();
    await kit.reply((url) => url === '/api/v1/resources/r1/days?from=2026-12-01&to=2026-12-31', DECEMBER);
    await kit.reply((url) => url === '/api/v1/resources/r1/exceptions?from=2026-12-01&to=2026-12-31', {
      items: [{ id: 'x1', day: '2026-12-23', kind: 'non_working', label: 'Training', version: 1 }],
      canManage,
    });
    const host = fixture.nativeElement as HTMLElement;
    const cell = (day: string) => host.querySelector<HTMLElement>(`[data-day="${day}"]`);
    return { ...kit, fixture, host, cell };
  }

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it('draws the month from Monday with holidays, closures and days not worked', async () => {
    const { host, cell, text, fixture } = await open();
    expect(text(host.querySelector('.month'))).toBe('December 2026');
    expect(host.querySelectorAll('tbody tr').length).toBe(5);
    expect(cell('2026-12-25')?.getAttribute('data-look')).toBe('holiday');
    expect([text(cell('2026-12-25')?.querySelector('.number')), text(cell('2026-12-25')?.querySelector('.tag'))]).toEqual(['25', 'Christmas']);
    expect(cell('2026-12-26')?.getAttribute('data-look')).toBe('off');
    // The name holds the visible text first (WCAG 2.5.3), then the details.
    expect(text(cell('2026-12-23'))).toBe('23 Training, Wednesday, December 23, 2026, closed, Exception of the resource');
    expect(text(cell('2026-12-21'))).toBe('21 7 h, Monday, December 21, 2026, worked');
    expect(cell('2026-12-21')?.hasAttribute('aria-label')).toBe(false);
    expect(Array.from(host.querySelectorAll('.legend li')).map((li) => text(li))).toEqual([
      'public holiday',
      'closed',
      'not worked',
      'absent',
      'Exception of the resource',
    ]);
    expect(text(host.querySelector('.exceptions li'))).toContain('Training');
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
  });

  it('moves with the arrow keys and opens the day’s exception with Enter', async () => {
    const { cell, settle, dialog, text, closed } = await open();
    // Only one day is in the tab order.
    expect(Array.from(document.querySelectorAll('[role="gridcell"][tabindex="0"]')).length).toBe(1);
    const start = cell('2026-12-16') as HTMLElement;
    start.focus();
    start.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
    await new Promise((resolve) => setTimeout(resolve, 30));
    await settle();
    expect(document.activeElement?.getAttribute('data-day')).toBe('2026-12-23');
    expect(cell('2026-12-23')?.getAttribute('tabindex')).toBe('0');
    (document.activeElement as HTMLElement).dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await settle();
    expect(text(dialog().querySelector('h2'))).toBe('Edit the exception');
    expect((dialog().querySelector('input[formcontrolname="label"]') as HTMLInputElement).value).toBe('Training');
    TestBed.inject(MatDialog).closeAll();
    await closed();
  });

  it('is read-only for a member', async () => {
    const { cell, host, settle } = await open(false);
    (cell('2026-12-21') as HTMLElement).click();
    await settle();
    expect(document.querySelector('mat-dialog-container')).toBeNull();
    expect(host.querySelector('.exceptions button')).toBeNull();
  });
});
