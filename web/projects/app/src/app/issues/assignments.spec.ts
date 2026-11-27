/**
 * Tests of assignments on a ticket (L08-US01): the value and error
 * helpers (unit), and the ticket's card with its dialog, the API mocked at
 * the HTTP level and the real translations (integration): the blocks per
 * resource and the planned total, assigning with several segments in
 * different modes, changing and removing, API errors on their fields,
 * read-only members, accessibility; and the overload check (L08-US03):
 * the load preview with overloaded days marked, debounced, the blocking
 * message and the "Force overload" button for those allowed, warning
 * mode, and the overload flagged on the ticket.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { FormArray, FormControl, FormGroup } from '@angular/forms';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter } from '@angular/router';
import { Assignment, AssignmentList, AssignmentMode, AssignmentPreview, Issue, Resource, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { PREVIEW_DEBOUNCE_MS, controlOf, segmentValue, valueText } from './assignment-dialog';
import { IssueAssignments, planSpan } from './issue-assignments';

const ISSUE = {
  id: 'i1',
  key: 'PLAT-148',
  summary: 'Account migration scripts',
  startDate: '2026-11-16',
  dueDate: '2026-11-27',
} as Issue;

const NATHAN: Assignment = {
  id: 'a1',
  issueId: 'i1',
  resource: { id: 'r1', kind: 'person', displayName: 'Nathan Roux', jobTitle: 'Sysadmin', active: true, linked: true },
  provisional: false,
  segments: [
    { start: '2026-11-16', end: '2026-11-20', mode: 'hours_per_day', value: 240, loadMinutes: 1200, workingDays: 5 },
    { start: '2026-11-23', end: '2026-11-27', mode: 'percentage', value: 30, loadMinutes: 630, workingDays: 5 },
  ],
  loadMinutes: 1830,
  outsideIssueDates: false,
  overloads: [],
  conflicts: [],
  version: 2,
};
const DEV: Assignment = {
  id: 'a2',
  issueId: 'i1',
  resource: { id: 'r2', kind: 'generic', displayName: 'Rust developer', active: true, linked: false },
  provisional: true,
  segments: [{ start: '2026-11-16', end: '2026-11-27', mode: 'total_effort', value: 2400, loadMinutes: 2400, workingDays: 10 }],
  loadMinutes: 2400,
  outsideIssueDates: true,
  overloads: [],
  conflicts: [],
  version: 1,
};
const CAMILLE = { id: 'r3', kind: 'person', displayName: 'Camille Martin', jobTitle: 'Back-end developer', active: true, teams: [] } as unknown as Resource;
const PATH = `/api/v1/issues/${ISSUE.id}/assignments`;
const PREVIEW = '/api/v1/assignments/preview';
const OVER = { date: '2026-11-18', plannedMinutes: 480, availableMinutes: 420, limitMinutes: 420 };

/** A preview of Nathan's week: 4 h existing on the 18th, 4 h/day added. */
function preview(overrides: Partial<AssignmentPreview> = {}): AssignmentPreview {
  const day = (date: string, existing: number, overloaded = false) => ({
    date,
    capacityMinutes: 420,
    limitMinutes: 420,
    existingMinutes: existing,
    assignmentMinutes: 240,
    totalMinutes: existing + 240,
    overloaded,
  });
  return {
    days: [day('2026-11-16', 120), day('2026-11-17', 180), day('2026-11-18', 240, true), day('2026-11-19', 180), day('2026-11-20', 150)],
    overloads: [OVER],
    mode: 'blocking',
    tolerancePercent: 0,
    countsProvisional: true,
    blocked: true,
    forceable: true,
    ...overrides,
  };
}
const QUIET = preview({ overloads: [], blocked: false, days: [] });

function problem(status: number, code: string, errors: object[] = []) {
  return { type: `urn:mlbpp:problem:${code}`, title: code, status, code, errors };
}

describe('assignment helpers', () => {
  it('read a value in each mode', () => {
    expect(segmentValue('percentage', '30')).toBe(30);
    expect(segmentValue('percentage', '30 %')).toBe(30);
    expect(segmentValue('percentage', '0')).toBeNull();
    expect(segmentValue('percentage', '101')).toBeNull();
    expect(segmentValue('percentage', '12.5')).toBeNull();
    expect(segmentValue('hours_per_day', '3,5')).toBe(210);
    expect(segmentValue('hours_per_day', '4:30')).toBe(270);
    expect(segmentValue('hours_per_day', '25')).toBeNull();
    expect(segmentValue('hours_per_day', '0')).toBeNull();
    expect(segmentValue('total_effort', '40')).toBe(2400);
    expect(segmentValue('total_effort', 'abc')).toBeNull();
    expect([valueText('percentage', 30), valueText('hours_per_day', 210), valueText('total_effort', 2400)]).toEqual(['30', '3:30', '40']);
  });

  it('find the field an API error names', () => {
    const segment = () =>
      new FormGroup({
        start: new FormControl<Date | null>(null),
        end: new FormControl<Date | null>(null),
        mode: new FormControl<AssignmentMode>('percentage', { nonNullable: true }),
        value: new FormControl('', { nonNullable: true }),
      });
    const segments = new FormArray([segment(), segment()]);
    const resource = new FormControl('');
    expect(controlOf('resourceId', segments, resource)).toBe(resource);
    expect(controlOf('segments.1.end', segments, resource)).toBe(segments.at(1).controls.end);
    expect(controlOf('segments.0.value', segments, resource)).toBe(segments.at(0).controls.value);
    expect(controlOf('segments.1', segments, resource)).toBe(segments.at(1).controls.start);
    expect(controlOf('segments.5.end', segments, resource)).toBeNull();
    expect(controlOf('segments', segments, resource)).toBeNull();
  });

  it('span the segments of every assignment', () => {
    expect(planSpan([NATHAN, DEV])).toEqual({ from: '2026-11-16', to: '2026-11-27' });
    expect(planSpan([])).toBeNull();
  });
});

describe('IssueAssignments', () => {
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
    /** Answers every preview asked for once the form is quiet. */
    const previews = async (body: AssignmentPreview = QUIET): Promise<TestRequest[]> => {
      await new Promise((resolve) => setTimeout(resolve, PREVIEW_DEBOUNCE_MS + 100));
      await settle();
      const requests = http.match((r) => r.urlWithParams === PREVIEW && r.method === 'POST');
      requests.forEach((request) => request.flush(body));
      await settle();
      return requests;
    };
    return { http, settle, reply, root, text, click, type, dialog, closed, previews };
  }

  const isList = (url: string, method: string) => url === PATH && method === 'GET';

  async function open(list: AssignmentList) {
    const kit = setup();
    const fixture = TestBed.createComponent(IssueAssignments);
    fixture.componentRef.setInput('issue', ISSUE);
    await kit.settle();
    await kit.reply(isList, list);
    const host = fixture.nativeElement as HTMLElement;
    const rows = (block: Element) => Array.from(block.querySelectorAll('tbody tr')).map((row) => Array.from(row.querySelectorAll('td')).map((td) => kit.text(td)));
    return { ...kit, fixture, host, rows };
  }

  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify();
  });

  it('shows a block per resource with its segments, and the planned total', async () => {
    const { host, text, rows, fixture } = await open({ items: [NATHAN, DEV], loadMinutes: 4230, canAssign: true });
    expect(text(host.querySelector('.subtitle'))).toBe('Planned total: 70 h 30 min · from Nov 16 to Nov 27');
    const blocks = Array.from(host.querySelectorAll('article.assignment'));
    expect(blocks.map((b) => text(b.querySelector('h3')))).toEqual(['Nathan Roux', 'Rust developer']);
    expect(text(blocks[0].querySelector('.load'))).toBe('30 h 30 min');
    expect(rows(blocks[0])).toEqual([
      ['Nov 16 → Nov 20, 2026', 'Hours per day', '4 h/day', '20 h'],
      ['Nov 23 → Nov 27, 2026', 'Percentage', '30%', '10 h 30 min'],
    ]);
    expect(rows(blocks[1])).toEqual([['Nov 16 → Nov 27, 2026', 'Total effort', '40 h over 10 working days', '40 h']]);
    expect(Array.from(blocks[1].querySelectorAll('.secondary .tag')).map((tag) => text(tag))).toEqual(['Generic resource', 'Provisional']);
    expect(text(blocks[1].querySelector('[role="note"]'))).toContain('Some segments fall outside the ticket’s dates');
    expect(blocks[0].querySelector('[role="note"]')).toBeNull();
    expect(Array.from(host.querySelectorAll('article button')).map((b) => b.getAttribute('aria-label'))).toEqual([
      'Edit Nathan Roux’s assignment',
      'Remove Nathan Roux from the ticket',
      'Confirm Rust developer’s provisional assignment',
      'Edit Rust developer’s assignment',
      'Remove Rust developer from the ticket',
    ]);
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
  });

  it('is read-only for those who do not assign, and says when nobody is assigned', async () => {
    const { host, text } = await open({ items: [], loadMinutes: 0, canAssign: false });
    expect(host.querySelector('#assignment-add')).toBeNull();
    expect(text(host.querySelector('.empty'))).toBe('No resource is assigned to this ticket.');
    expect(host.querySelector('.subtitle')).toBeNull();
  });

  it('assigns a resource with two segments in different modes', async () => {
    const { host, root, click, type, reply, dialog, closed, settle, text, previews } = await open({ items: [], loadMinutes: 0, canAssign: true });
    await click(host.querySelector('#assignment-add'));
    await reply((url) => url.startsWith('/api/v1/resources?') && url.includes('active=true'), { items: [CAMILLE], nextCursor: null, canManage: false });
    expect(text(dialog().querySelector('h2'))).toBe('Assign a resource');
    expect(text(dialog().querySelector('.ticket'))).toBe('PLAT-148 · Account migration scripts · from Nov 16 to Nov 27');
    expect(await axeViolations(dialog())).toEqual([]);

    // Nothing chosen nor typed: both are required.
    await click(dialog().querySelector('button[type="submit"]'));
    expect(Array.from(dialog().querySelectorAll('mat-error')).map((e) => text(e))).toContain('This field is required.');

    // The failed submission left the focus on its button: the search opens on a focused field.
    dialog().querySelector<HTMLInputElement>('input.resource-input')?.focus();
    await type(dialog().querySelector('input.resource-input'), 'cam');
    await new Promise((resolve) => setTimeout(resolve, 300));
    await settle();
    await reply((url) => url.startsWith('/api/v1/resources?') && url.includes('search=cam'), { items: [CAMILLE], nextCursor: null, canManage: false });
    const panel = Array.from(root().querySelectorAll('.mat-mdc-autocomplete-panel')).at(-1);
    await click(Array.from(panel?.querySelectorAll('mat-option') ?? []).find((o) => o.textContent?.includes('Camille')) ?? null);

    // The first segment covers the ticket's dates, in hours per day.
    const segments = () => Array.from(dialog().querySelectorAll('li.segment'));
    expect((segments()[0].querySelector('input[matStartDate]') as HTMLInputElement).value).toBe('11/16/2026');
    expect((segments()[0].querySelector('input[matEndDate]') as HTMLInputElement).value).toBe('11/27/2026');
    await type(segments()[0].querySelector('input[matEndDate]'), '11/20/2026');
    await type(segments()[0].querySelector('input[formcontrolname="value"]'), '4');
    // A second segment starts the day after, in the same mode, switched to %.
    await click(dialog().querySelector('button.add-segment'));
    expect((segments()[1].querySelector('input[matStartDate]') as HTMLInputElement).value).toBe('11/21/2026');
    const percent = Array.from(segments()[1].querySelectorAll('mat-button-toggle button')).find((b) => text(b) === '%');
    await click(percent ?? null);
    expect(text(segments()[1].querySelector('mat-hint'))).toBe('Share of each day’s capacity');
    await type(segments()[1].querySelector('input[formcontrolname="value"]'), '130');
    await click(dialog().querySelector('button[type="submit"]'));
    expect(text(segments()[1].querySelector('mat-error'))).toBe('A percentage from 1 to 100.');
    await type(segments()[1].querySelector('input[formcontrolname="value"]'), '30');
    // A third one, removed again.
    await click(dialog().querySelector('button.add-segment'));
    expect(segments().length).toBe(3);
    await click(segments()[2].querySelector('button.remove'));
    expect(segments().length).toBe(2);
    const asked = await previews();
    expect(asked.at(-1)?.request.body).toMatchObject({ issueId: ISSUE.id, resourceId: CAMILLE.id, provisional: false });
    expect(await axeViolations(dialog())).toEqual([]);

    await click(dialog().querySelector('button[type="submit"]'));
    const created = await reply((url, method) => url === PATH && method === 'POST', { ...NATHAN, resource: { ...NATHAN.resource, displayName: 'Camille Martin' } }, 201);
    expect(created.request.body).toEqual({
      resourceId: CAMILLE.id,
      provisional: false,
      force: false,
      segments: [
        { start: '2026-11-16', end: '2026-11-20', mode: 'hours_per_day', value: 240 },
        { start: '2026-11-21', end: '2026-11-27', mode: 'percentage', value: 30 },
      ],
    });
    expect(created.request.headers.get('Idempotency-Key')).toBeTruthy();
    await closed();
    await reply(isList, { items: [NATHAN], loadMinutes: 1830, canAssign: true });
    expect(text(host.querySelector('[role="status"]'))).toBe('Camille Martin was assigned to the ticket.');
  });

  it('shows API errors on the fields they name, and above the form', async () => {
    const { host, http, click, type, reply, dialog, text, closed, previews } = await open({ items: [NATHAN], loadMinutes: 1830, canAssign: true });
    await click(host.querySelector(`#assignment-edit-${NATHAN.id}`));
    await previews();
    expect(text(dialog().querySelector('h2'))).toBe('Edit the assignment');
    expect(text(dialog().querySelector('.resource-fixed span:last-child'))).toBe('Nathan Roux');
    const values = () => Array.from(dialog().querySelectorAll<HTMLInputElement>('input[formcontrolname="value"]')).map((input) => input.value);
    expect(values()).toEqual(['4', '30']);

    await click(dialog().querySelector('button[type="submit"]'));
    const refused = await reply(
      (url, method) => url === `/api/v1/assignments/${NATHAN.id}` && method === 'PUT',
      problem(409, 'assignment.outside_issue_dates', [{ field: 'segments.1.end', code: 'outside_issue_dates' }]),
      409,
    );
    expect(refused.request.headers.get('If-Match')).toBe('"2"');
    expect(refused.request.body).toMatchObject({ provisional: false });
    expect(text(dialog().querySelector('[role="alert"]'))).toBe('Segments must stay within the ticket’s dates.');
    expect(text(dialog().querySelectorAll('li.segment')[1].querySelector('mat-error'))).toBe('Outside the ticket’s dates.');
    // The field keeps the error until it changes.
    await click(dialog().querySelector('button[type="submit"]'));
    expect(http.match(() => true).length).toBe(0);
    await type(dialog().querySelectorAll('li.segment')[1].querySelector('input[matEndDate]') ?? null, '11/26/2026');
    await previews();

    await click(dialog().querySelector('button[type="submit"]'));
    await reply(
      (url, method) => url === `/api/v1/assignments/${NATHAN.id}` && method === 'PUT',
      problem(400, 'validation_failed', [{ field: 'segments.0.start', code: 'no_working_day' }]),
      400,
    );
    expect(dialog().querySelector('[role="alert"]')).toBeNull();
    expect(text(dialog().querySelectorAll('li.segment')[0].querySelector('mat-error'))).toBe('The resource has no working day in this period.');
    // Closed here: closed by afterEach, it would give focus back after teardown.
    TestBed.inject(MatDialog).closeAll();
    await closed();
  });

  it('removes an assignment after confirmation', async () => {
    const { host, click, reply, dialog, closed, text } = await open({ items: [NATHAN], loadMinutes: 1830, canAssign: true });
    await click(host.querySelector('article button[aria-label^="Remove"]'));
    expect(text(dialog().querySelector('h2'))).toBe('Remove Nathan Roux?');
    await click(Array.from(dialog().querySelectorAll('button')).find((b) => text(b) === 'Remove') ?? null);
    await closed();
    const deleted = await reply((url, method) => url === `/api/v1/assignments/${NATHAN.id}` && method === 'DELETE', null, 204);
    expect(deleted.request.headers.get('If-Match')).toBe('"2"');
    await reply(isList, { items: [], loadMinutes: 0, canAssign: true });
    expect(text(host.querySelector('[role="status"]'))).toBe('Nathan Roux was removed from the ticket.');
  });

  it('previews the load per day, blocks an overload and forces it for those allowed', async () => {
    const { host, click, reply, dialog, text, closed, previews, settle } = await open({ items: [NATHAN], loadMinutes: 1830, canAssign: true });
    await click(host.querySelector(`#assignment-edit-${NATHAN.id}`));
    const [asked] = await previews(preview());
    expect(asked.request.body).toEqual({
      issueId: ISSUE.id,
      resourceId: NATHAN.resource.id,
      assignmentId: NATHAN.id,
      provisional: false,
      segments: [
        { start: '2026-11-16', end: '2026-11-20', mode: 'hours_per_day', value: 240 },
        { start: '2026-11-23', end: '2026-11-27', mode: 'percentage', value: 30 },
      ],
    });
    expect(text(dialog().querySelector('#preview-title'))).toBe('Load preview of Nathan Roux');
    const rows = Array.from(dialog().querySelectorAll('.preview-table tbody tr'));
    expect(rows.map((row) => text(row.querySelector('th')))).toEqual(['Capacity', 'Existing load', 'This assignment', 'Total']);
    const totals = Array.from(rows[3].querySelectorAll('td'));
    expect(totals.map((cell) => text(cell))).toEqual(['6 h', '7 h', '8 h warningoverloaded', '7 h', '6 h 30 min']);
    // Overloaded in text, not only in color.
    expect(totals.map((cell) => cell.classList.contains('over'))).toEqual([false, false, true, false, false]);
    expect(text(totals[2].querySelector('.cdk-visually-hidden'))).toBe('overloaded');
    expect(text(dialog().querySelector('.message.overload[role="alert"]'))).toBe(
      'block Capacity exceeded — Wed, Nov 18: 8 h planned for 7 h available (+1 h). The assignment cannot be saved as it is: reduce its segments or force the overload.',
    );
    const save = dialog().querySelector<HTMLButtonElement>('button[type="submit"]');
    expect(save?.disabled).toBe(true);
    expect(text(dialog().querySelector('.force-note'))).toContain('reserved for roles with this permission');
    expect(await axeViolations(dialog())).toEqual([]);

    await click(dialog().querySelector('button.force'));
    const forced = await reply((url, method) => url === `/api/v1/assignments/${NATHAN.id}` && method === 'PUT', { ...NATHAN, overloads: [OVER], version: 3 });
    expect(forced.request.body).toMatchObject({ force: true, provisional: false });
    await closed();
    await reply(isList, { items: [{ ...NATHAN, overloads: [OVER] }], loadMinutes: 1830, canAssign: true });
    await settle();
  });

  it('refuses an overload without the permission to force it, until a segment changes', async () => {
    const { host, http, click, type, reply, dialog, text, closed, previews } = await open({ items: [NATHAN], loadMinutes: 1830, canAssign: true });
    await click(host.querySelector(`#assignment-edit-${NATHAN.id}`));
    await previews();
    // The provisional switch is sent with the assignment.
    await click(dialog().querySelector('mat-slide-toggle button'));
    const [asked] = await previews();
    expect(asked.request.body).toMatchObject({ provisional: true });

    await click(dialog().querySelector('button[type="submit"]'));
    const refused = await reply(
      (url, method) => url === `/api/v1/assignments/${NATHAN.id}` && method === 'PUT',
      { ...problem(409, 'workload.overload'), overload: { days: [OVER, { ...OVER, date: '2026-11-19' }], forceable: false } },
      409,
    );
    expect(refused.request.body).toMatchObject({ provisional: true, force: false });
    expect(text(dialog().querySelector('[role="alert"]'))).toBe(
      'block Capacity exceeded — Wed, Nov 18: 8 h planned for 7 h available (+1 h); Thu, Nov 19: 8 h planned for 7 h available (+1 h). The assignment cannot be saved as it is: reduce its segments.',
    );
    expect(dialog().querySelector('button.force')).toBeNull();
    expect(dialog().querySelector<HTMLButtonElement>('button[type="submit"]')?.disabled).toBe(true);

    // A change answers it: saving is possible again once previewed.
    await type(dialog().querySelector('li.segment input[formcontrolname="value"]'), '3');
    expect(dialog().querySelector('[role="alert"]')).toBeNull();
    expect(dialog().querySelector<HTMLButtonElement>('button[type="submit"]')?.disabled).toBe(false);
    await previews();
    expect(http.match(() => true).length).toBe(0);
    TestBed.inject(MatDialog).closeAll();
    await closed();
  });

  it('asks for the preview once the form is quiet', async () => {
    const { host, http, click, type, dialog, closed, previews, settle } = await open({ items: [NATHAN], loadMinutes: 1830, canAssign: true });
    await click(host.querySelector(`#assignment-edit-${NATHAN.id}`));
    await previews();
    const value = () => dialog().querySelector('li.segment input[formcontrolname="value"]');
    await type(value(), '5');
    await type(value(), '6');
    await type(value(), '7');
    await new Promise((resolve) => setTimeout(resolve, PREVIEW_DEBOUNCE_MS / 2));
    await settle();
    expect(http.match((r) => r.urlWithParams === PREVIEW).length).toBe(0);
    const asked = await previews();
    expect(asked.length).toBe(1);
    expect(asked[0].request.body).toMatchObject({ segments: [{ value: 420 }, { value: 30 }] });
    // An incomplete segment asks for nothing.
    await type(value(), '');
    expect((await previews()).length).toBe(0);
    TestBed.inject(MatDialog).closeAll();
    await closed();
  });

  it('warns of an overload in warning mode and saves it', async () => {
    const { host, click, reply, dialog, text, closed, previews } = await open({ items: [NATHAN], loadMinutes: 1830, canAssign: true });
    await click(host.querySelector(`#assignment-edit-${NATHAN.id}`));
    await previews(preview({ mode: 'warning', blocked: false }));
    expect(text(dialog().querySelector('.message.overload[role="status"]'))).toBe(
      'warning Capacity exceeded — Wed, Nov 18: 8 h planned for 7 h available (+1 h). The assignment will be saved and the overload flagged.',
    );
    expect(dialog().querySelector('button.force')).toBeNull();
    expect(dialog().querySelector<HTMLButtonElement>('button[type="submit"]')?.disabled).toBe(false);
    expect(await axeViolations(dialog())).toEqual([]);
    await click(dialog().querySelector('button[type="submit"]'));
    await reply((url, method) => url === `/api/v1/assignments/${NATHAN.id}` && method === 'PUT', { ...NATHAN, overloads: [OVER], version: 3 });
    await closed();
    await reply(isList, { items: [{ ...NATHAN, overloads: [OVER] }], loadMinutes: 1830, canAssign: true });
  });

  it('lists the hours an absence leaves no room for, to reschedule', async () => {
    const conflicts = [
      { date: '2026-11-18', minutes: 240 },
      { date: '2026-11-19', minutes: 30 },
    ];
    const { host, text, fixture } = await open({ items: [{ ...NATHAN, conflicts }, DEV], loadMinutes: 4230, canAssign: true });
    const blocks = Array.from(host.querySelectorAll('article.assignment'));
    const note = blocks[0].querySelector('.conflicts');
    expect(text(note?.querySelector('.conflicts-title'))).toBe('4 h 30 min to reschedule: the resource is away');
    expect(Array.from(note?.querySelectorAll('li') ?? []).map((li) => text(li))).toEqual(['Wed, Nov 18: 4 h', 'Thu, Nov 19: 0 h 30 min']);
    expect(note?.getAttribute('role')).toBe('note');
    expect(blocks[1].querySelector('.conflicts')).toBeNull();
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
  });

  it('flags an assignment taking part in an overload', async () => {
    const { host, text, fixture } = await open({ items: [{ ...NATHAN, overloads: [OVER] }, DEV], loadMinutes: 4230, canAssign: true });
    const blocks = Array.from(host.querySelectorAll('article.assignment'));
    expect(Array.from(blocks[0].querySelectorAll('.secondary .tag')).map((tag) => text(tag))).toEqual(['warning Overload']);
    expect(text(blocks[0].querySelector('[role="note"]'))).toBe('warning Resource overloaded: Wed, Nov 18: 8 h planned for 7 h available (+1 h).');
    expect(blocks[1].querySelector('.tag.overload')).toBeNull();
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
  });

  const isConfirm = (url: string, method: string) => url === `/api/v1/assignments/${DEV.id}/confirm` && method === 'POST';

  it('confirms a provisional assignment, which then reads as firm', async () => {
    const { host, click, reply, text } = await open({ items: [NATHAN, DEV], loadMinutes: 4230, canAssign: true });
    expect(host.querySelector(`#assignment-confirm-${NATHAN.id}`)).toBeNull();
    const button = host.querySelector(`#assignment-confirm-${DEV.id}`);
    expect(text(button)).toBe('check_circle Confirm');
    await click(button);
    const confirmed = await reply(isConfirm, { ...DEV, provisional: false, version: 2 });
    expect(confirmed.request.headers.get('If-Match')).toBe('"1"');
    expect(confirmed.request.body).toEqual({ force: false });
    await reply(isList, { items: [NATHAN, { ...DEV, provisional: false, version: 2 }], loadMinutes: 4230, canAssign: true });
    expect(text(host.querySelector('[role="status"]'))).toBe('Rust developer’s assignment is now firm.');
    expect(host.querySelector('.tag.provisional')).toBeNull();
    expect(host.querySelector('button.confirm')).toBeNull();
  });

  it('asks to force the overload confirming makes, for those allowed', async () => {
    const { host, click, reply, dialog, text, closed } = await open({ items: [DEV], loadMinutes: 2400, canAssign: true });
    await click(host.querySelector(`#assignment-confirm-${DEV.id}`));
    await reply(isConfirm, { ...problem(409, 'workload.overload'), overload: { days: [OVER], forceable: true } }, 409);
    expect(text(dialog().querySelector('h2'))).toBe('Force the overload?');
    expect(text(dialog().querySelector('mat-dialog-content'))).toBe(
      'Confirmed, this assignment goes over capacity — Wed, Nov 18: 8 h planned for 7 h available (+1 h). Forcing the overload is traced.',
    );
    expect(await axeViolations(dialog())).toEqual([]);
    await click(Array.from(dialog().querySelectorAll('button')).find((b) => text(b) === 'Confirm anyway') ?? null);
    await closed();
    const forced = await reply(isConfirm, { ...DEV, provisional: false, overloads: [OVER], version: 2 });
    expect(forced.request.body).toEqual({ force: true });
    await reply(isList, { items: [{ ...DEV, provisional: false, overloads: [OVER], version: 2 }], loadMinutes: 2400, canAssign: true });
    expect(text(host.querySelector('[role="status"]'))).toBe('Rust developer’s assignment is now firm.');
  });

  it('leaves the assignment provisional when forcing is declined', async () => {
    const { host, http, click, reply, dialog, text, closed } = await open({ items: [DEV], loadMinutes: 2400, canAssign: true });
    await click(host.querySelector(`#assignment-confirm-${DEV.id}`));
    await reply(isConfirm, { ...problem(409, 'workload.overload'), overload: { days: [OVER], forceable: true } }, 409);
    await click(Array.from(dialog().querySelectorAll('button')).find((b) => text(b) === 'Cancel') ?? null);
    await closed();
    expect(http.match(() => true).length).toBe(0);
    expect(host.querySelector('[role="status"], [role="alert"]')).toBeNull();
    expect(host.querySelector(`#assignment-confirm-${DEV.id}`)).not.toBeNull();
  });

  it('tells the days confirming would overload, without the permission to force', async () => {
    const { host, click, reply, text } = await open({ items: [DEV], loadMinutes: 2400, canAssign: true });
    await click(host.querySelector(`#assignment-confirm-${DEV.id}`));
    await reply(isConfirm, { ...problem(409, 'workload.overload'), overload: { days: [OVER], forceable: false } }, 409);
    expect(document.querySelector('mat-dialog-container')).toBeNull();
    expect(text(host.querySelector('[role="alert"]'))).toBe(
      'Confirmed, this assignment would go over capacity — Wed, Nov 18: 8 h planned for 7 h available (+1 h). It stays provisional: reduce its segments, or ask someone allowed to force the overload.',
    );
  });

  it('reads the assignments again when the ticket changed elsewhere', async () => {
    const { fixture, reply, settle } = await open({ items: [], loadMinutes: 0, canAssign: true });
    fixture.componentRef.setInput('refresh', 1);
    await settle();
    await reply(isList, { items: [NATHAN], loadMinutes: 1830, canAssign: true });
    expect((fixture.nativeElement as HTMLElement).querySelectorAll('article').length).toBe(1);
  });
});
