/**
 * Tests of absences (L07-US05): how a span reads and the window shown
 * (unit), and the absences card with its dialog, the API mocked at the
 * HTTP level and the real translations (integration): the list with its
 * declarer and comment, declaring a half day, an overlap refused, editing
 * and deleting, read-only for others, accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter } from '@angular/router';
import { Absence, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { AbsencesCard, absenceSpan, absenceWindow } from './absences-card';

const LEAVE: Absence = {
  id: 'a1',
  kind: 'leave',
  start: '2026-12-04',
  startsAtNoon: false,
  end: '2026-12-04',
  endsAtNoon: false,
  comment: 'Family',
  declaredBy: { accountId: 'me', displayName: 'Camille Martin' },
  conflictMinutes: 0,
  version: 1,
};
const TRIP: Absence = {
  ...LEAVE,
  id: 'a2',
  kind: 'training',
  start: '2026-12-28',
  startsAtNoon: true,
  end: '2026-12-31',
  endsAtNoon: false,
  comment: undefined,
  declaredBy: { accountId: 'm', displayName: 'Lucas Bernard' },
  version: 2,
};

function problem(status: number, code: string, errors: object[] = []) {
  return { type: `urn:mlbpp:problem:${code}`, title: code, status, code, errors };
}

describe('absence helpers', () => {
  it('read a day, a half day or a range', () => {
    expect(absenceSpan(LEAVE)).toEqual({ single: true, part: 'whole' });
    expect(absenceSpan({ ...LEAVE, startsAtNoon: true })).toEqual({ single: true, part: 'afternoon' });
    expect(absenceSpan({ ...LEAVE, endsAtNoon: true })).toEqual({ single: true, part: 'morning' });
    expect(absenceSpan(TRIP)).toEqual({ single: false, fromNoon: true, toNoon: false });
  });

  it('show a month back and the rest of the year ahead', () => {
    expect(absenceWindow(new Date(2026, 10, 24))).toEqual({ from: '2026-10-25', to: '2027-10-24' });
  });
});

describe('AbsencesCard', () => {
  async function open(items: readonly Absence[], canManage = true) {
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
    const fixture = TestBed.createComponent(AbsencesCard);
    fixture.componentRef.setInput('resourceId', 'r1');
    const changes: number[] = [];
    fixture.componentInstance.changed.subscribe(() => changes.push(1));
    await settle();
    const list = await reply((url, method) => url.startsWith('/api/v1/resources/r1/absences?') && method === 'GET', { items, canManage });
    const host = fixture.nativeElement as HTMLElement;
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
    return { fixture, host, http, list, reply, text, click, type, dialog, closed, changes };
  }

  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify();
  });

  it('lists the absences with their dates, half days, declarer and comment', async () => {
    const { host, text, list, fixture } = await open([LEAVE, TRIP]);
    expect(list.request.params.get('from')).toBeTruthy();
    const rows = Array.from(host.querySelectorAll('.absence'));
    expect(rows.map((row) => text(row.querySelector('.kind')))).toEqual(['Leave', 'Training']);
    expect(text(rows[0].querySelector('.when'))).toBe('Fri, Dec 4 · whole day · declared by Camille Martin');
    expect(text(rows[1].querySelector('.when'))).toBe('From Dec 28 to Dec 31, 2026 · from noon · declared by Lucas Bernard');
    expect(text(rows[0].querySelector('.comment'))).toBe('Family');
    expect(rows[1].querySelector('.comment')).toBeNull();
    expect(Array.from(host.querySelectorAll('.actions button')).map((b) => b.getAttribute('aria-label'))).toEqual([
      'Edit the absence of Dec 4, 2026',
      'Delete the absence of Dec 4, 2026',
      'Edit the absence of Dec 28, 2026',
      'Delete the absence of Dec 28, 2026',
    ]);
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
  });

  it('shows the hours in conflict on an absence, in text and not only in color', async () => {
    const { host, text, fixture } = await open([{ ...LEAVE, conflictMinutes: 450 }, TRIP]);
    const rows = Array.from(host.querySelectorAll('.absence'));
    expect(text(rows[0].querySelector('.conflict'))).toBe('event_busy 7 h 30 min in conflict');
    expect(text(rows[0].querySelector('.kind'))).toBe('Leave event_busy 7 h 30 min in conflict');
    expect(rows[1].querySelector('.conflict')).toBeNull();
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
  });

  it('declares a half day of training, and tells an overlap', async () => {
    const { host, click, type, reply, dialog, closed, text, changes } = await open([]);
    expect(text(host.querySelector('.empty'))).toBe('No absence from a month ago to a year ahead.');
    await click(host.querySelector('#absence-new'));
    expect(text(dialog().querySelector('h2'))).toBe('Declare an absence');
    expect(await axeViolations(dialog())).toEqual([]);
    await click(dialog().querySelector('mat-select .mat-mdc-select-trigger'));
    await click(Array.from(document.body.querySelectorAll('mat-option')).find((o) => o.textContent?.includes('Training')) ?? null);
    await type(dialog().querySelector('input[formcontrolname="start"]'), '12/7/2026');
    // The last day follows the first.
    expect((dialog().querySelector('input[formcontrolname="end"]') as HTMLInputElement).value).toBe('12/7/2026');
    await click(dialog().querySelector('mat-slide-toggle[formcontrolname="startsAtNoon"] button'));
    await type(dialog().querySelector('textarea'), ' Rust ');
    await click(dialog().querySelector('button[type="submit"]'));
    const refused = await reply(
      (url, method) => url === '/api/v1/resources/r1/absences' && method === 'POST',
      problem(409, 'absence.overlap', [{ field: 'start', code: 'overlap' }]),
      409,
    );
    expect(refused.request.body).toEqual({
      kind: 'training',
      start: '2026-12-07',
      startsAtNoon: true,
      end: '2026-12-07',
      endsAtNoon: false,
      comment: 'Rust',
    });
    expect(text(dialog().querySelector('mat-error'))).toBe('Another absence takes this half day.');

    // Another day: the error goes with the change.
    await type(dialog().querySelector('input[formcontrolname="start"]'), '12/8/2026');
    await click(dialog().querySelector('button[type="submit"]'));
    const created = await reply((url, method) => url === '/api/v1/resources/r1/absences' && method === 'POST', { ...TRIP, id: 'a3' }, 201);
    expect(created.request.body).toMatchObject({ start: '2026-12-08', end: '2026-12-08' });
    expect(created.request.headers.get('Idempotency-Key')).toBeTruthy();
    await closed();
    await reply((url, method) => url.startsWith('/api/v1/resources/r1/absences?') && method === 'GET', { items: [TRIP], canManage: true });
    expect(text(host.querySelector('[role="status"]'))).toBe('The absence was declared.');
    expect(changes.length, 'the month grid reads its days again').toBe(1);
  });

  it('changes an absence with its version and deletes one after confirmation', async () => {
    const { host, click, reply, dialog, closed, text } = await open([LEAVE]);
    await click(host.querySelector('#absence-edit-a1'));
    expect(text(dialog().querySelector('h2'))).toBe('Edit the absence');
    await click(dialog().querySelector('mat-slide-toggle[formcontrolname="endsAtNoon"] button'));
    await click(dialog().querySelector('button[type="submit"]'));
    const saved = await reply((url, method) => url === '/api/v1/resources/r1/absences/a1' && method === 'PUT', { ...LEAVE, endsAtNoon: true, version: 2 });
    expect(saved.request.headers.get('If-Match')).toBe('"1"');
    expect(saved.request.body).toMatchObject({ endsAtNoon: true, comment: 'Family' });
    await closed();
    await reply((url, method) => url.startsWith('/api/v1/resources/r1/absences?') && method === 'GET', { items: [{ ...LEAVE, endsAtNoon: true, version: 2 }], canManage: true });

    await click(host.querySelector('#absence-delete-a1'));
    await click(Array.from(dialog().querySelectorAll('button')).find((b) => text(b) === 'Delete') ?? null);
    await closed();
    const deleted = await reply((url, method) => url === '/api/v1/resources/r1/absences/a1' && method === 'DELETE', null, 204);
    expect(deleted.request.headers.get('If-Match')).toBe('"2"');
    await reply((url, method) => url.startsWith('/api/v1/resources/r1/absences?') && method === 'GET', { items: [], canManage: true });
    expect(text(host.querySelector('[role="status"]'))).toBe('The absence was deleted.');
  });

  it('is read-only for others, who never see the comment', async () => {
    const { host } = await open([{ ...LEAVE, comment: undefined }], false);
    expect(host.querySelector('#absence-new')).toBeNull();
    expect(host.querySelector('.actions')).toBeNull();
    expect(host.querySelector('.comment')).toBeNull();
  });
});
