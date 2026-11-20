/**
 * Tests of the bulk action dialogs and report (L06-US08), the API mocked
 * at the HTTP level and the real translations: the statuses a transition
 * reaches offered with their reach, only what is chosen sent by Modify,
 * one plan for a batch move with its suggestions, the report of each
 * ticket, and accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { Type } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter } from '@angular/router';
import { BulkAction, BulkOptions, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { BulkDialogData } from './bulk-flow';
import { BulkModifyDialog } from './bulk-modify-dialog';
import { BulkMoveDialog } from './bulk-move-dialog';
import { BulkReport } from './bulk-report';
import { BulkStatusDialog } from './bulk-status-dialog';

const SELECTION: BulkDialogData = { selection: { issueIds: ['i1', 'i2', 'i3'] }, count: 3 };

const OPTIONS: BulkOptions = {
  total: 3,
  background: false,
  projectKey: 'PLAT',
  fields: [
    {
      fieldId: 'f-points',
      key: 'story_points',
      label: 'Points',
      kind: 'number',
      mode: 'visible',
      decimals: 1,
    } as never,
  ],
  statuses: [
    { key: 'done', label: 'Done', category: 'done', reachable: 1 },
    { key: 'in_review', label: 'In review', category: 'in_progress', reachable: 2 },
  ],
};

function report(overrides: Partial<BulkAction> = {}): BulkAction {
  return {
    id: 'b1',
    kind: 'transition',
    status: 'done',
    background: false,
    total: 3,
    counts: { changed: 1, unchanged: 1, refused: 1, withParent: 0, waiting: 0 },
    items: [
      { issueId: 'i1', key: 'PLAT-1', outcome: 'changed', errors: [] },
      { issueId: 'i2', key: 'PLAT-2', outcome: 'unchanged', errors: [] },
      { issueId: 'i3', key: 'OPS-1', outcome: 'refused', reason: 'transition.restricted', errors: [] },
    ],
    createdAt: '2026-11-19T09:00:00Z',
    completedAt: '2026-11-19T09:00:01Z',
    expiresAt: '2026-11-26T09:00:00Z',
    ...overrides,
  };
}

describe('the bulk action dialogs', () => {
  async function open(component: Type<unknown>) {
    TestBed.configureTestingModule({
      providers: [provideRouter([]), provideApi(), provideHttpClientTesting(), provideI18nTesting(), provideMaterialDefaults()],
    });
    const http = TestBed.inject(HttpTestingController);
    const settle = async () => {
      for (let i = 0; i < 3; i++) {
        await new Promise((resolve) => setTimeout(resolve, 20));
        TestBed.tick();
      }
    };
    const reply = async (match: (url: string, method: string) => boolean, body: object | null, status = 200): Promise<TestRequest> => {
      let request: TestRequest | undefined;
      for (let i = 0; i < 50 && !request; i++) {
        [request] = http.match((r) => match(r.urlWithParams, r.method));
        if (!request) await new Promise((resolve) => setTimeout(resolve, 20));
      }
      if (!request) throw new Error('no matching request');
      request.flush(body, { status, statusText: 'S', headers: status >= 400 ? { 'content-type': 'application/problem+json' } : {} });
      await settle();
      return request;
    };
    const ref = TestBed.inject(MatDialog).open(component, { data: SELECTION });
    const closed: unknown[] = [];
    ref.beforeClosed().subscribe((value) => closed.push(value));
    await settle();
    const pane = () => document.querySelector('mat-dialog-container') as HTMLElement;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const click = async (element: Element | null | undefined) => {
      if (!element) throw new Error('nothing to click');
      (element as HTMLElement).click();
      await settle();
    };
    return { http, reply, settle, pane, text, click, closed };
  }

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it('changes the status to one a transition reaches, with how many tickets it reaches', async () => {
    const { pane, text, click, reply, closed } = await open(BulkStatusDialog);
    const asked = await reply((url, method) => url === '/api/v1/bulk-actions/options' && method === 'POST', OPTIONS);
    expect(asked.request.body).toEqual({ issueIds: ['i1', 'i2', 'i3'] });
    expect(text(pane().querySelector('h2'))).toBe('Change the status of 3 tickets');
    const radios = Array.from(pane().querySelectorAll('mat-radio-button')).map((r) => text(r));
    expect(radios).toEqual(['Done 1 of 3', 'In review 2 of 3']);
    expect((pane().querySelector('button.confirm') as HTMLButtonElement).disabled).toBe(true);
    await click(pane().querySelectorAll('mat-radio-button input')[1]);
    expect(await axeViolations(pane())).toEqual([]);
    await click(pane().querySelector('button.confirm'));
    const run = await reply((url, method) => url === '/api/v1/bulk-actions' && method === 'POST', report());
    expect(run.request.body).toEqual({ selection: { issueIds: ['i1', 'i2', 'i3'] }, action: { kind: 'transition', statusKey: 'in_review' } });
    expect(closed).toEqual([report()]);
  });

  it('sends only what Modify is told to change', async () => {
    const { pane, click, reply, settle, closed, text } = await open(BulkModifyDialog);
    await reply((url) => url === '/api/v1/bulk-actions/options', OPTIONS);
    // Nothing chosen: nothing sent.
    await click(pane().querySelector('button.confirm'));
    expect(text(pane().querySelector('[role="alert"]'))).toBe('Choose something to change.');
    // Priority high, assignee cleared, the points set to 3.
    await click(pane().querySelector('mat-select.priority'));
    await click(Array.from(document.querySelectorAll('mat-option')).find((o) => o.textContent?.includes('High') && !o.textContent.includes('Highest')));
    await click(pane().querySelector('mat-select.assignee-mode'));
    await click(Array.from(document.querySelectorAll('mat-option')).find((o) => o.textContent?.trim() === 'Clear'));
    await click(pane().querySelector('mat-checkbox.change-field input'));
    const points = pane().querySelector('app-field-input input') as HTMLInputElement;
    points.value = '3';
    points.dispatchEvent(new Event('input'));
    await settle();
    expect(await axeViolations(pane())).toEqual([]);
    await click(pane().querySelector('button.confirm'));
    const run = await reply((url, method) => url === '/api/v1/bulk-actions' && method === 'POST', report({ kind: 'modify' }));
    expect(run.request.body).toEqual({
      selection: { issueIds: ['i1', 'i2', 'i3'] },
      action: { kind: 'modify', priority: 'high', assigneeId: null, fields: { 'f-points': 3 } },
    });
    expect(closed.length).toBe(1);
  });

  it('moves with one plan for the batch, its suggestions prefilled', async () => {
    const { pane, click, reply, text, closed } = await open(BulkMoveDialog);
    await reply((url) => url.startsWith('/api/v1/nodes?'), {
      items: [{ id: 'p2', type: 'project', key: 'SALES', name: 'Sales', archived: false }],
      nextCursor: null,
    });
    await click(pane().querySelector('mat-select.project'));
    await click(document.querySelector('mat-option'));
    const plan = {
      project: { id: 'p2', key: 'SALES', name: 'Sales' },
      sameConfiguration: false,
      tickets: 2,
      subtasksAlone: 1,
      targets: [{ id: 'tt', label: 'Task', isSubtask: false, parentIds: [], statusIds: ['st'] }],
      statusTargets: [{ id: 'st', label: 'To do', category: 'todo' }],
      types: [{ id: 'ts', key: 'task', label: 'Task', isSubtask: false, suggestedId: 'tt' }],
      statuses: [{ id: 'ss', key: 'todo', label: 'To do', category: 'todo', typeIds: ['ts'], suggestedId: 'st' }],
      droppedFields: [],
      problems: [],
    };
    const first = await reply((url) => url === '/api/v1/bulk-actions/move-plan', plan);
    expect(first.request.body).toEqual({ selection: { issueIds: ['i1', 'i2', 'i3'] }, projectKey: 'SALES', typeMappings: [], statusMappings: [] });
    // The suggestions mapped, the plan asked again with them.
    const again = await reply((url) => url === '/api/v1/bulk-actions/move-plan', plan);
    expect((again.request.body as { typeMappings: unknown[] }).typeMappings).toEqual([{ fromId: 'ts', toId: 'tt' }]);
    await reply((url) => url === '/api/v1/bulk-actions/move-plan', plan).catch(() => undefined);
    expect(text(pane().querySelector('.plan .note'))).toBe('2 tickets move (their sub-tasks follow them).');
    expect(text(pane().querySelector('.plan .warning'))).toBe('1 sub-tasks selected without their parent stay where they are.');
    expect(await axeViolations(pane())).toEqual([]);
    await click(pane().querySelector('button.confirm'));
    const run = await reply((url, method) => url === '/api/v1/bulk-actions' && method === 'POST', report({ kind: 'move' }));
    expect(run.request.body).toEqual({
      selection: { issueIds: ['i1', 'i2', 'i3'] },
      action: { kind: 'move', projectKey: 'SALES', typeMappings: [{ fromId: 'ts', toId: 'tt' }], statusMappings: [{ fromId: 'ss', toId: 'st' }] },
    });
    expect(closed.length).toBe(1);
  });
});

describe('the report of a bulk action', () => {
  it('tells what became of each ticket', async () => {
    TestBed.configureTestingModule({ providers: [provideRouter([]), provideI18nTesting()] });
    const fixture = TestBed.createComponent(BulkReport);
    fixture.componentRef.setInput(
      'report',
      report({
        kind: 'move',
        counts: { changed: 1, unchanged: 0, refused: 1, withParent: 1, waiting: 1 },
        items: [
          { issueId: 'i1', key: 'PLAT-1', newKey: 'CRM-1', outcome: 'changed', errors: [] },
          { issueId: 'i2', key: 'PLAT-2', outcome: 'with_parent', errors: [] },
          { issueId: 'i3', key: 'PLAT-4', outcome: 'refused', reason: 'validation_failed', errors: [{ field: 'fields.x', code: 'required' }] },
          { issueId: 'i4', key: 'PLAT-5', outcome: null, errors: [] },
        ],
      }),
    );
    await fixture.whenStable();
    const root = fixture.nativeElement as HTMLElement;
    const rows = Array.from(root.querySelectorAll('tbody tr')).map((r) =>
      Array.from(r.querySelectorAll('td'))
        .map((cell) => cell.textContent?.replace(/\s+/g, ' ').trim())
        .join(' '),
    );
    expect(rows).toEqual([
      'PLAT-1 Moved: CRM-1',
      'PLAT-2 Moved with its parent',
      'PLAT-4 A value is refused (fields.x)',
      'PLAT-5 Waiting',
    ]);
    expect(root.querySelector('a')?.getAttribute('href')).toBe('/issues/CRM-1');
    expect(root.querySelector('.counts')?.textContent?.replace(/\s+/g, ' ').trim()).toBe(
      '2 changed, 0 already so, 1 left unchanged · 1 waiting',
    );
    expect(await axeViolations(root)).toEqual([]);
  });
});
