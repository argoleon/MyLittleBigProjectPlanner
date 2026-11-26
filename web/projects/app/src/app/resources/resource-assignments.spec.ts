/**
 * Tests of the "Assignments" tab of a resource (L08-US05): grouping by
 * project with busy time last (unit), and the tab with the API mocked at
 * the HTTP level and the real translations (integration): the groups and
 * their rows, busy time without ticket, past assignments on demand,
 * editing through the assignment dialog, removing after confirmation,
 * accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter } from '@angular/router';
import { Assignment, Issue, ResourceAssignment, ResourceAssignmentPage, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { PREVIEW_DEBOUNCE_MS } from '../issues/assignment-dialog';
import { ResourceAssignments, groupByProject } from './resource-assignments';

const PLAT = { id: 'p1', key: 'PLAT', name: 'Identity platform' };
const OPS = { id: 'p2', key: 'OPS', name: 'Operations' };

function item(id: string, rest: Partial<ResourceAssignment> = {}): ResourceAssignment {
  return {
    id,
    busy: false,
    issueId: `i-${id}`,
    issue: { id: `i-${id}`, key: 'PLAT-148', summary: 'Account migration scripts', project: PLAT },
    provisional: false,
    segments: [{ start: '2026-11-16', end: '2026-11-20', mode: 'hours_per_day', value: 240, loadMinutes: 1200, workingDays: 5 }],
    loadMinutes: 1200,
    overloads: [],
    conflicts: [],
    canEdit: true,
    version: 2,
    ...rest,
  };
}

const MIGRATION = item('a1');
const LATER = item('a2', {
  issue: { id: 'i-a2', key: 'PLAT-151', summary: 'MFA policy', project: PLAT },
  segments: [{ start: '2026-12-07', end: '2026-12-11', mode: 'percentage', value: 50, loadMinutes: 1050, workingDays: 5 }],
  loadMinutes: 1050,
  provisional: true,
  conflicts: [{ date: '2026-12-09', minutes: 210 }],
});
const SYNC = item('a3', {
  issue: { id: 'i-a3', key: 'OPS-12', summary: 'Directory sync', project: OPS },
  canEdit: false,
  overloads: [{ date: '2026-11-18', plannedMinutes: 480, availableMinutes: 420, limitMinutes: 420 }],
});
const BUSY = item('a4', { busy: true, issueId: null, issue: undefined, canEdit: false, loadMinutes: 600 });

describe('groupByProject', () => {
  it('groups by project name, each by its first day, busy time last', () => {
    const groups = groupByProject([BUSY, SYNC, LATER, MIGRATION]);
    expect(groups.map((g) => g.name)).toEqual(['Identity platform', 'Operations', null]);
    expect(groups[0].items.map((i) => i.id)).toEqual(['a1', 'a2']);
    expect(groups[0].loadMinutes).toBe(2250);
    expect(groups[2].id).toBeNull();
    expect(groupByProject([])).toEqual([]);
  });
});

describe('ResourceAssignments', () => {
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
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const click = async (element: Element | null) => {
      if (!element) throw new Error('nothing to click');
      (element as HTMLElement).click();
      await settle();
    };
    const dialog = () => Array.from(document.body.querySelectorAll('mat-dialog-container')).at(-1) ?? document.body;
    const closed = async () => {
      await new Promise((resolve) => setTimeout(resolve, 200));
      await settle();
    };
    return { http, settle, reply, text, click, dialog, closed };
  }

  const isList = (url: string, method: string) => url.startsWith('/api/v1/resources/r1/assignments?') && method === 'GET';

  async function open(page: ResourceAssignmentPage) {
    const kit = setup();
    const fixture = TestBed.createComponent(ResourceAssignments);
    fixture.componentRef.setInput('resourceId', 'r1');
    await kit.settle();
    const list = await kit.reply(isList, page);
    const host = fixture.nativeElement as HTMLElement;
    return { ...kit, fixture, host, list };
  }

  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify();
  });

  it('lists the assignments by project, busy time last, with their periods and load', async () => {
    const { host, text, list, fixture } = await open({ items: [MIGRATION, LATER, SYNC, BUSY], nextCursor: null });
    // Ongoing and upcoming, with their tickets.
    expect(list.request.params.get('from')).toMatch(/^\d{4}-\d{2}-\d{2}$/);
    expect(list.request.params.get('expand')).toBe('issue');
    const groups = Array.from(host.querySelectorAll('section.group'));
    expect(groups.map((g) => text(g.querySelector('.group-title a') ?? g.querySelector('.group-title')))).toEqual(['Identity platform', 'Operations', 'Busy']);
    expect(groups.map((g) => text(g.querySelector('.project-key')))).toEqual(['PLAT', 'OPS', undefined]);
    expect(groups.map((g) => text(g.querySelector('.group-load')))).toEqual(['37 h 30 min', '20 h', '10 h']);
    const rows = Array.from(groups[0].querySelectorAll('tbody tr'));
    expect([text(rows[0].querySelector('.key')), text(rows[0].querySelector('.summary'))]).toEqual(['PLAT-148', 'Account migration scripts']);
    expect(text(rows[0].querySelector('.periods'))).toBe('Nov 16 → Nov 20, 2026 · 4 h/day');
    expect(Array.from(rows[1].querySelectorAll('.tag')).map((t) => text(t))).toEqual(['Provisional', 'Absence conflict']);
    expect(text(rows[1].querySelector('.periods'))).toBe('Dec 7 → Dec 11, 2026 · 50%');
    expect(text(groups[1].querySelector('.tags'))).toBe('Overload');
    // Read-only where the member does not assign; busy time has no ticket.
    expect(groups[1].querySelectorAll('.actions button')).toHaveLength(0);
    expect(text(groups[2].querySelector('.busy'))).toBe('Busy');
    expect(groups[2].querySelector('a')).toBeNull();
    expect(text(groups[2].querySelector('.busy-hint'))).toContain('only the periods and hours show');
    expect(Array.from(groups[0].querySelectorAll('.actions button')).map((b) => b.getAttribute('aria-label'))).toEqual([
      'Edit the assignment on PLAT-148',
      'Remove the assignment on PLAT-148',
      'Edit the assignment on PLAT-151',
      'Remove the assignment on PLAT-151',
    ]);
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
  });

  it('shows past assignments on demand, and says when there is none', async () => {
    const { host, text, click, reply } = await open({ items: [], nextCursor: null });
    expect(text(host.querySelector('.empty'))).toBe('No ongoing or upcoming assignment.');
    await click(host.querySelector('mat-slide-toggle button'));
    const all = await reply(isList, { items: [MIGRATION], nextCursor: 'next' });
    expect(all.request.params.has('from')).toBe(false);
    expect(host.querySelectorAll('tbody tr')).toHaveLength(1);
    await click(Array.from(host.querySelectorAll('button')).find((b) => text(b) === 'Show more') ?? null);
    const more = await reply(isList, { items: [SYNC], nextCursor: null });
    expect(more.request.params.get('cursor')).toBe('next');
    expect(host.querySelectorAll('tbody tr')).toHaveLength(2);
  });

  it('edits an assignment through the assignment dialog, with its ticket', async () => {
    const { host, click, reply, dialog, text, closed, http, settle } = await open({ items: [MIGRATION], nextCursor: null });
    await click(host.querySelector('#resource-assignment-edit-a1'));
    const stored: Assignment = {
      id: 'a1',
      issueId: 'i-a1',
      resource: { id: 'r1', kind: 'generic', displayName: 'Nathan Roux', active: true, linked: false },
      provisional: false,
      segments: MIGRATION.segments,
      loadMinutes: 1200,
      outsideIssueDates: false,
      overloads: [],
      conflicts: [],
      version: 2,
    };
    await reply((url) => url === '/api/v1/assignments/a1', stored);
    await reply((url) => url.startsWith('/api/v1/issues/i-a1'), { id: 'i-a1', key: 'PLAT-148', summary: 'Account migration scripts' } as Issue);
    expect(text(dialog().querySelector('h2'))).toBe('Edit the assignment');
    // The dialog's load preview.
    await new Promise((resolve) => setTimeout(resolve, PREVIEW_DEBOUNCE_MS + 100));
    await settle();
    http.match((r) => r.urlWithParams === '/api/v1/assignments/preview').forEach((r) => r.flush({ days: [], overloads: [], mode: 'blocking', tolerancePercent: 0, countsProvisional: true, blocked: false, forceable: true }));
    await settle();
    await click(dialog().querySelector('button[type="submit"]'));
    const saved = await reply((url, method) => url === '/api/v1/assignments/a1' && method === 'PUT', { ...stored, version: 3 });
    expect(saved.request.headers.get('If-Match')).toBe('"2"');
    await closed();
    await reply(isList, { items: [{ ...MIGRATION, version: 3 }], nextCursor: null });
    expect(text(host.querySelector('[role="status"]'))).toBe('The assignment on PLAT-148 was saved.');
  });

  it('removes an assignment after confirmation', async () => {
    const { host, click, reply, dialog, text, closed } = await open({ items: [MIGRATION], nextCursor: null });
    await click(host.querySelector('button[aria-label="Remove the assignment on PLAT-148"]'));
    expect(text(dialog().querySelector('h2'))).toBe('Remove the assignment on PLAT-148?');
    await click(Array.from(dialog().querySelectorAll('button')).find((b) => text(b) === 'Remove') ?? null);
    await closed();
    const deleted = await reply((url, method) => url === '/api/v1/assignments/a1' && method === 'DELETE', null, 204);
    expect(deleted.request.headers.get('If-Match')).toBe('"2"');
    await reply(isList, { items: [], nextCursor: null });
    expect(text(host.querySelector('[role="status"]'))).toBe('The assignment on PLAT-148 was removed.');
  });
});
