/**
 * Tests of capacity (L07-US03): the hours fields (unit), and the capacity
 * card of a resource, its period dialog and the workload settings of the
 * administration, with the API mocked at the HTTP level and the real
 * translations (integration): periods and default, hours as planners read
 * them, add, change, delete, a date already taken, rights, accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter } from '@angular/router';
import { CapacityPattern, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { WorkloadSettingsTab } from '../admin/workload-settings';
import { CapacityCard } from './capacity-card';
import { hoursText, hoursValidator, weekHoursForm, weekHoursValue } from './week-hours';

const RESOURCE = '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b01';
const PATH = `/api/v1/resources/${RESOURCE}/capacity-patterns`;
const DEFAULT = [420, 420, 420, 420, 420, 0, 0];
const JANUARY: CapacityPattern = {
  id: 'p1',
  effectiveFrom: '2026-01-01',
  minutes: [420, 420, 420, 420, 420, 0, 0],
  weeklyMinutes: 2100,
  label: 'Full time',
  version: 1,
};
const LATER: CapacityPattern = {
  id: 'p2',
  effectiveFrom: '2099-03-01',
  minutes: [420, 420, 0, 420, 210, 0, 0],
  weeklyMinutes: 1470,
  label: 'Part time 80%',
  version: 2,
};

function problem(status: number, code: string, errors: object[] = []) {
  return { type: `urn:mlbpp:problem:${code}`, title: code, status, code, errors };
}

describe('week hours', () => {
  it('types minutes back as hours, reads hours as minutes', () => {
    expect(hoursText(420)).toBe('7');
    expect(hoursText(210)).toBe('3:30');
    expect(hoursText(0)).toBe('0');
    const form = weekHoursForm([420, 210, 0, 0, 0, 0, 0]);
    expect(form.controls.map((c) => c.value)).toEqual(['7', '3:30', '0', '0', '0', '0', '0']);
    form.at(2).setValue('3,5');
    form.at(3).setValue('');
    expect(weekHoursValue(form)).toEqual([420, 210, 210, 0, 0, 0, 0]);
    form.at(4).setValue('soon');
    expect(form.at(4).hasError('hours')).toBe(true);
    expect(hoursValidator(form.at(0))).toBeNull();
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
      if (!request) await new Promise((resolve) => setTimeout(resolve, 20));
    }
    if (!request) throw new Error(`no matching request among ${http.match(() => true).map((r) => r.request.urlWithParams).join(', ') || 'none'}`);
    request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error' });
    await settle();
    return request;
  };
  const root = () => document.body;
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
  const dialog = () => Array.from(root().querySelectorAll('mat-dialog-container')).at(-1) ?? root();
  const closed = async () => {
    await new Promise((resolve) => setTimeout(resolve, 200));
    await settle();
  };
  return { http, settle, reply, root, text, click, type, dialog, closed };
}

const isList = (url: string, method: string) => url === PATH && method === 'GET';

describe('CapacityCard', () => {
  async function open(items: readonly CapacityPattern[], canManage = true) {
    const kit = setup();
    const fixture = TestBed.createComponent(CapacityCard);
    fixture.componentRef.setInput('resourceId', RESOURCE);
    await kit.settle();
    await kit.reply(isList, { items, defaultMinutes: DEFAULT, canManage });
    const host = fixture.nativeElement as HTMLElement;
    const rows = () => Array.from(host.querySelectorAll('tr[mat-row]'));
    const cells = (row: Element) => Array.from(row.querySelectorAll('td')).map((cell) => kit.text(cell));
    const changes = { count: 0 };
    fixture.componentInstance.changed.subscribe(() => changes.count++);
    return { ...kit, fixture, host, rows, cells, changes };
  }

  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify();
  });

  it('lists each period with its hours per weekday and weekly total', async () => {
    const { host, rows, cells, text, fixture } = await open([JANUARY, LATER]);
    expect(Array.from(host.querySelectorAll('th abbr')).map((abbr) => text(abbr))).toEqual(['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']);
    expect(rows().map((row) => cells(row).slice(1, 9))).toEqual([
      ['7 h', '7 h', '7 h', '7 h', '7 h', '–No hours', '–No hours', '35 h'],
      ['7 h', '7 h', '–No hours', '7 h', '3 h 30 min', '–No hours', '–No hours', '24 h 30 min'],
    ]);
    expect(text(rows()[0].querySelector('.from'))).toBe('Jan 1, 2026');
    expect(text(rows()[0].querySelector('.secondary'))).toBe('Full time');
    expect(text(rows()[1].querySelector('.secondary'))).toBe('Part time 80% · planned');
    expect(text(host.querySelector('.default'))).toBe('Before Jan 1, 2026: the organization default (35 h a week).');
    expect(Array.from(host.querySelectorAll('td button')).map((b) => b.getAttribute('aria-label'))).toEqual([
      'Edit the period from Jan 1, 2026',
      'Delete the period from Jan 1, 2026',
      'Edit the period from Mar 1, 2099',
      'Delete the period from Mar 1, 2099',
    ]);
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
  });

  it('shows the organization default while there is no period, read-only for members', async () => {
    const { host, rows, cells, text } = await open([], false);
    expect(rows().length).toBe(1);
    expect(text(rows()[0].querySelector('.from'))).toBe('Organization default');
    expect(cells(rows()[0]).slice(1)).toEqual(['7 h', '7 h', '7 h', '7 h', '7 h', '–No hours', '–No hours', '35 h']);
    expect(host.querySelector('button.new')).toBeNull();
    expect(host.querySelector('td button')).toBeNull();
  });

  it('adds a period from the latest week, typed in hours', async () => {
    const { host, click, type, reply, dialog, closed, text, changes } = await open([JANUARY]);
    await click(host.querySelector('button.new'));
    expect(text(dialog().querySelector('h2'))).toBe('New capacity period');
    const days = () => Array.from(dialog().querySelectorAll<HTMLInputElement>('app-week-hours input'));
    expect(days().map((input) => input.value)).toEqual(['7', '7', '7', '7', '7', '0', '0']);
    expect(await axeViolations(dialog())).toEqual([]);
    // The date is required.
    await click(dialog().querySelector('button[type="submit"]'));
    expect(text(dialog().querySelector('mat-error'))).toBe('This field is required.');

    await type(dialog().querySelector('input[formcontrolname="effectiveFrom"]'), '3/1/2026');
    await type(days()[2], '0');
    await type(days()[4], '3,5');
    await type(dialog().querySelector('input[formcontrolname="label"]'), ' Part time 80% ');
    expect(text(dialog().querySelector('app-week-hours .hint'))).toContain('Week: 24 h 30 min');
    await click(dialog().querySelector('button[type="submit"]'));
    const created = await reply((url, method) => url === PATH && method === 'POST', { ...LATER, effectiveFrom: '2026-03-01' }, 201);
    expect(created.request.body).toEqual({
      effectiveFrom: '2026-03-01',
      minutes: [420, 420, 0, 420, 210, 0, 0],
      label: 'Part time 80%',
    });
    expect(created.request.headers.get('Idempotency-Key')).toBeTruthy();
    await closed();
    await reply(isList, { items: [JANUARY, LATER], defaultMinutes: DEFAULT, canManage: true });
    expect(text(host.querySelector('[role="status"]'))).toBe('The period was added.');
    // The month and the weeks of the resource page read their days again.
    expect(changes.count).toBe(1);
  });

  it('tells a date already taken on the date field, and refuses hours that are not', async () => {
    const { host, click, type, reply, dialog, text } = await open([JANUARY]);
    await click(host.querySelector(`#period-edit-${JANUARY.id}`));
    expect(text(dialog().querySelector('h2'))).toBe('Edit the capacity period');
    const days = Array.from(dialog().querySelectorAll<HTMLInputElement>('app-week-hours input'));
    await type(days[0], '25');
    await click(dialog().querySelector('button[type="submit"]'));
    expect(text(dialog().querySelector('app-week-hours mat-error'))).toBe('Hours of a day, such as 7, 3.5 or 3:30.');
    await type(days[0], '8');
    await click(dialog().querySelector('button[type="submit"]'));
    const refused = await reply(
      (url, method) => url === `${PATH}/${JANUARY.id}` && method === 'PUT',
      problem(409, 'capacity_pattern.date_taken', [{ field: 'effectiveFrom', code: 'taken' }]),
      409,
    );
    expect(refused.request.headers.get('If-Match')).toBe('"1"');
    expect(refused.request.body).toMatchObject({ effectiveFrom: '2026-01-01', minutes: [480, 420, 420, 420, 420, 0, 0] });
    expect(text(dialog().querySelector('[role="alert"]'))).toBe('The resource already has a capacity period starting that day.');
    expect(text(dialog().querySelector('mat-form-field mat-error'))).toBe('This value is already used.');
  });

  it('deletes a period after confirmation', async () => {
    const { host, click, reply, dialog, closed, text, changes } = await open([JANUARY, LATER]);
    await click(host.querySelector(`#period-delete-${LATER.id}`));
    expect(text(dialog().querySelector('h2'))).toBe('Delete this period?');
    await click(Array.from(dialog().querySelectorAll('button')).find((b) => text(b) === 'Delete') ?? null);
    await closed();
    const deleted = await reply((url, method) => url === `${PATH}/${LATER.id}` && method === 'DELETE', null, 204);
    expect(deleted.request.headers.get('If-Match')).toBe('"2"');
    await reply(isList, { items: [JANUARY], defaultMinutes: DEFAULT, canManage: true });
    expect(text(host.querySelector('[role="status"]'))).toBe('The period was deleted.');
    expect(changes.count).toBe(1);
  });
});

describe('WorkloadSettingsTab', () => {
  const SETTINGS = '/api/v1/tenant/workload-settings';

  async function open() {
    const kit = setup();
    const fixture = TestBed.createComponent(WorkloadSettingsTab);
    await kit.settle();
    await kit.reply((url, method) => url === SETTINGS && method === 'GET', {
      defaultCapacityMinutes: DEFAULT,
      overloadTolerancePercent: 0,
      overloadMode: 'blocking',
      overloadCountsProvisional: true,
      version: 4,
    });
    return { ...kit, fixture, host: fixture.nativeElement as HTMLElement };
  }

  afterEach(() => TestBed.inject(HttpTestingController).verify());

  it('changes the default capacity, the tolerance and the mode with the tenant version', async () => {
    const { host, type, click, reply, text, fixture } = await open();
    const days = Array.from(host.querySelectorAll<HTMLInputElement>('app-week-hours input'));
    expect(days.map((input) => input.value)).toEqual(['7', '7', '7', '7', '7', '0', '0']);
    expect(text(host.querySelector('.choice-hint'))).toContain('The assignment is refused');
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
    expect((host.querySelector('button[type="submit"]') as HTMLButtonElement).disabled).toBe(true);

    await type(days[4], '3.5');
    await type(host.querySelector('input[formcontrolname="tolerance"]'), '10');
    await click(host.querySelector('mat-button-toggle[value="warning"] button'));
    expect(text(host.querySelector('.choice-hint'))).toContain('The assignment is accepted');
    // Provisional assignments counted (the default), switched off.
    const provisional = host.querySelector<HTMLButtonElement>('mat-slide-toggle button');
    expect(provisional?.getAttribute('aria-checked')).toBe('true');
    expect(text(host.querySelector('mat-slide-toggle'))).toContain('Count provisional assignments in the overload check');
    await click(provisional);
    await click(host.querySelector('button[type="submit"]'));
    const saved = await reply((url, method) => url === SETTINGS && method === 'PUT', {
      defaultCapacityMinutes: [420, 420, 420, 420, 210, 0, 0],
      overloadTolerancePercent: 10,
      overloadMode: 'warning',
      overloadCountsProvisional: false,
      version: 5,
    });
    expect(saved.request.headers.get('If-Match')).toBe('"4"');
    expect(saved.request.body).toEqual({
      defaultCapacityMinutes: [420, 420, 420, 420, 210, 0, 0],
      overloadTolerancePercent: 10,
      overloadMode: 'warning',
      overloadCountsProvisional: false,
    });
    expect(host.querySelector('mat-slide-toggle button')?.getAttribute('aria-checked')).toBe('false');
    expect(text(host.querySelector('[role="status"]'))).toBe('The workload settings were saved.');
  });

  it('places the errors of the API on their fields', async () => {
    const { host, type, click, reply, text } = await open();
    await type(host.querySelector('input[formcontrolname="tolerance"]'), '5');
    await click(host.querySelector('button[type="submit"]'));
    await reply(
      (url, method) => url === SETTINGS && method === 'PUT',
      problem(400, 'validation_failed', [{ field: 'defaultCapacityMinutes.6', code: 'out_of_range' }]),
      400,
    );
    expect(text(host.querySelector('[role="alert"]'))).toBe('Some fields are invalid.');
    expect(text(host.querySelectorAll('app-week-hours mat-form-field')[6].querySelector('mat-error'))).toBe(
      'The value is out of the allowed range.',
    );
  });
});
