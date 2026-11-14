/**
 * Tests of the Workflows tab of a configuration (L04-US04): the layout of
 * the read-only graph, the API errors placed on their fields, then the tab
 * in the configuration page with the API mocked at the HTTP level and the
 * real translations: the workflows by issue type, the problems, the graph
 * and the tables, the status and transition dialogs, removals.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { FormControl, FormGroup } from '@angular/forms';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import {
  ApiProblem,
  AuthService,
  ProjectConfig,
  Workflow,
  WorkflowStatus,
  WorkflowTransition,
  provideApi,
  provideMaterialDefaults,
  translationsForm,
} from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { ConfigPage } from './config-page';
import { placeApiErrors } from './form-errors';
import { NODE, chipCategory, layoutWorkflow } from './workflow-layout';
import { WorkflowsTab } from './workflows-tab';

const CONFIG: ProjectConfig = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a02',
  name: 'Software development',
  nameTranslations: { fr: 'Développement logiciel', en: 'Software development' },
  isDefault: false,
  projectCount: 6,
  published: { number: 2, publishedAt: '2026-11-12T09:00:00Z' },
  draft: null,
  version: 5,
};

const status = (id: string, key: string, fr: string, en: string, category: WorkflowStatus['category'], isInitial = false): WorkflowStatus => ({
  id,
  key,
  label: en,
  labelTranslations: { fr, en },
  portalLabel: null,
  portalLabelTranslations: {},
  category,
  isInitial,
  version: 1,
});

const transition = (id: string, en: string, fromId: string | null, toId: string): WorkflowTransition => ({
  id,
  name: en,
  nameTranslations: { en },
  fromId,
  toId,
  rules: { conditions: { roleIds: [], groupIds: [] }, validators: [], postFunctions: [] },
  version: 1,
});

/** The standard workflow of mockup 11. */
const STANDARD: Workflow = {
  id: 'w1',
  key: 'standard',
  label: 'Standard workflow',
  labelTranslations: { fr: 'Workflow standard', en: 'Standard workflow' },
  issueTypeIds: ['t2', 't3'],
  statuses: [
    status('s1', 'todo', 'À faire', 'To do', 'todo', true),
    status('s2', 'in_progress', 'En cours', 'In progress', 'in_progress'),
    status('s3', 'in_review', 'En revue', 'In review', 'in_progress'),
    status('s4', 'done', 'Terminé', 'Done', 'done'),
    status('s5', 'cancelled', 'Annulé', 'Cancelled', 'done'),
  ],
  transitions: [
    transition('x1', 'Start', 's1', 's2'),
    transition('x2', 'Send to review', 's2', 's3'),
    transition('x3', 'Reject', 's3', 's2'),
    transition('x4', 'Validate', 's3', 's4'),
    transition('x5', 'Cancel', null, 's5'),
  ],
  problems: [],
  version: 1,
};

/** A workflow being built: a status nothing leads to. */
const BUG: Workflow = {
  ...STANDARD,
  id: 'w2',
  key: 'bug',
  label: 'Bug workflow',
  labelTranslations: { fr: 'Workflow bug', en: 'Bug workflow' },
  issueTypeIds: ['t4'],
  statuses: [status('b1', 'open', 'Ouvert', 'Open', 'todo', true), status('b2', 'fixed', 'Corrigé', 'Fixed', 'done')],
  transitions: [],
  problems: [{ code: 'unreachable', statusId: 'b2', transitionId: null }],
};

const TYPES = {
  stage: 'published',
  items: [
    { id: 't2', key: 'story', label: 'Story', labelTranslations: { fr: 'Story', en: 'Story' }, icon: 'bookmark', color: 'story', isSubtask: false, parentIds: [], workflowId: 'w1', level: 1, version: 1 },
    { id: 't3', key: 'task', label: 'Task', labelTranslations: { fr: 'Tâche', en: 'Task' }, icon: 'task_alt', color: 'task', isSubtask: false, parentIds: [], workflowId: 'w1', level: 1, version: 1 },
    { id: 't4', key: 'bug', label: 'Bug', labelTranslations: { fr: 'Bug', en: 'Bug' }, icon: 'bug_report', color: 'bug', isSubtask: false, parentIds: [], workflowId: 'w2', level: 1, version: 1 },
  ],
};

const SESSION = {
  account: { id: 'a1', email: 'lucas@acme.example', displayName: 'Lucas Bernard', locale: null, tenantRole: 'configuration_admin' },
  stage: 'complete',
  csrfToken: 'csrf',
  expiresAt: '2026-11-20T09:00:00Z',
};

function problem(status: number, code: string, errors: object[] = []) {
  return { type: `urn:mlbpp:problem:${code}`, title: code, status, code, errors };
}

describe('the layout of a workflow graph', () => {
  it('places the statuses in columns from the initial one, the others below', () => {
    const layout = layoutWorkflow(STANDARD);
    const at = (id: string) => layout.statuses.find((s) => s.status.id === id)!;
    const step = NODE.width + NODE.gapX;
    expect([at('s1').x, at('s2').x, at('s3').x, at('s4').x].map((x) => (x - NODE.margin) / step)).toEqual([0, 1, 2, 3]);
    // Cancelled is reached from any status only: below, dashed.
    expect(at('s5').apart).toBe(true);
    expect(at('s5').y).toBeGreaterThan(at('s1').y);
    expect(layout.statuses.map((s) => s.category)).toEqual(['todo', 'progress', 'progress', 'done', 'done']);
    expect(layout.width).toBe(NODE.margin + 3 * step + NODE.width + NODE.margin);
  });

  it('draws forward, backward and from-any transitions apart', () => {
    const layout = layoutWorkflow(STANDARD);
    const edge = (id: string) => layout.transitions.find((t) => t.transition.id === id)!;
    expect(edge('x1').path).toMatch(/^M \d+ \d+ L /);
    expect(edge('x3').path).toContain(' C ');
    expect(edge('x5').fromAny).toBe(true);
    expect(layout.transitions.length).toBe(5);
  });

  it('places everything apart without an initial status', () => {
    const layout = layoutWorkflow({ statuses: STANDARD.statuses.map((s) => ({ ...s, isInitial: false })), transitions: [] });
    expect(layout.statuses.every((s) => s.apart)).toBe(true);
    expect(layoutWorkflow({ statuses: [], transitions: [] }).statuses).toEqual([]);
    expect(chipCategory('in_progress')).toBe('progress');
  });
});

describe('placeApiErrors', () => {
  it('puts each error on its field, a label in one or every language', () => {
    const form = new FormGroup({ key: new FormControl(''), labelTranslations: translationsForm({ fr: 'A' }) });
    const taken = new ApiProblem(409, 'workflow.key_taken', 'taken', null, [{ field: 'key', code: 'taken' }]);
    expect(placeApiErrors(taken, form.controls)).toBeNull();
    expect(form.controls.key.getError('api')).toBe('errors.field.taken');
    const long = new ApiProblem(400, 'validation_failed', 'invalid', null, [{ field: 'labelTranslations.en', code: 'too_long' }]);
    expect(placeApiErrors(long, form.controls)).toBeNull();
    expect(form.controls.labelTranslations.controls.en.getError('api')).toBe('errors.field.too_long');
    expect(form.controls.labelTranslations.controls.fr.getError('api')).toBeNull();
  });

  it('leaves the rest above the form', () => {
    const form = new FormGroup({ key: new FormControl('') });
    expect(placeApiErrors(new ApiProblem(409, 'workflow.in_use', 'in use', null, []), form.controls)).toBe('errors.workflow.in_use');
    const elsewhere = new ApiProblem(400, 'validation_failed', 'invalid', null, [{ field: 'copyOf', code: 'not_found' }]);
    expect(placeApiErrors(elsewhere, form.controls)).toBe('errors.validation_failed');
    expect(placeApiErrors(new Error('boom'), form.controls)).toBe('errors.unknown');
  });
});

describe('WorkflowsTab', () => {
  async function openWorkflows(items: Workflow[] = [STANDARD, BUG]) {
    localStorage.clear();
    TestBed.configureTestingModule({
      providers: [
        provideRouter(
          [{ path: 'admin/configurations/:id', component: ConfigPage, children: [{ path: 'workflows', component: WorkflowsTab }] }],
          withComponentInputBinding(),
        ),
        provideApi(),
        provideHttpClientTesting(),
        provideI18nTesting(),
        provideMaterialDefaults(),
      ],
    });
    const http = TestBed.inject(HttpTestingController);
    const settle = async () => {
      for (let i = 0; i < 3; i++) {
        await new Promise((resolve) => setTimeout(resolve));
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
      request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error' });
      await settle();
      return request;
    };
    const loaded = TestBed.inject(AuthService).load();
    await reply((url) => url === '/api/v1/auth/session', SESSION);
    await loaded;
    const harness = await RouterTestingHarness.create();
    await harness.navigateByUrl(`/admin/configurations/${CONFIG.id}/workflows`);
    await reply((url) => url === `/api/v1/project-configs/${CONFIG.id}`, CONFIG);
    await reply((url) => url.endsWith('/workflows'), { stage: 'published', items });
    await reply((url) => url.endsWith('/issue-types'), TYPES);
    const root = () => document.body;
    const tab = () => root().querySelector('app-workflows-tab')!;
    const dialog = () => Array.from(root().querySelectorAll('mat-dialog-container')).at(-1)!;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const texts = (selector: string, within: Element = tab()) => Array.from(within.querySelectorAll(selector)).map((e) => text(e));
    const click = async (element: Element | null) => {
      (element as HTMLElement).click();
      await settle();
    };
    const fill = async (selector: string, value: string) => {
      const input = dialog().querySelector(selector) as HTMLInputElement;
      input.value = value;
      input.dispatchEvent(new Event('input'));
      input.dispatchEvent(new Event('blur'));
      await settle();
    };
    const choose = async (index: number, label: string) => {
      await click(dialog().querySelectorAll('mat-select .mat-mdc-select-trigger')[index]);
      await click(Array.from(root().querySelectorAll('mat-option')).find((o) => text(o) === label)!);
    };
    const waitFor = async (done: () => boolean) => {
      for (let i = 0; i < 50 && !done(); i++) {
        await new Promise((resolve) => setTimeout(resolve, 20));
        await settle();
      }
    };
    return { reply, root, tab, dialog, text, texts, click, fill, choose, waitFor };
  }

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it('lists the workflows by issue type, and shows the first with its graph and tables', async () => {
    const { tab, text, texts } = await openWorkflows();
    expect(texts('.item .item-types')).toEqual(['Story, Task', 'Bug']);
    expect(texts('.item .item-problems')).toEqual(['To fix before publishing']);
    expect(tab().querySelector('.item.current')?.getAttribute('aria-current')).toBe('true');
    expect(text(tab().querySelector('#workflow-title'))).toBe('Standard workflow standard');
    expect(tab().querySelector('.problems')).toBeNull();
    const graph = tab().querySelector('app-workflow-graph svg')!;
    expect(graph.getAttribute('role')).toBe('img');
    expect(graph.getAttribute('aria-label')).toBe('Graph of Standard workflow; the statuses and transitions tables below describe it.');
    expect(graph.querySelectorAll('g[data-category]').length).toBe(5);
    const statuses = Array.from(tab().querySelectorAll('table.statuses tr[mat-row]'));
    expect(statuses.map((row) => text(row.querySelector('td strong')))).toEqual(['À faire', 'En cours', 'En revue', 'Terminé', 'Annulé']);
    expect(statuses.map((row) => text(row.querySelector('td [lang="en"]')))).toEqual(['To do', 'In progress', 'In review', 'Done', 'Cancelled']);
    expect(statuses.map((row) => text(row.querySelector('.initial')) ?? null)).toEqual(['Initial', null, null, null, null]);
    expect(texts('table.statuses ui-status-chip')).toEqual([
      'Status: To do',
      'Status: In progress',
      'Status: In progress',
      'Status: Done',
      'Status: Done',
    ]);
    // The arrow is hidden from screen readers, which read "to" instead.
    expect(texts('table.transitions td.move')).toEqual([
      'To do →to In progress',
      'In progress →to In review',
      'In review →to In progress',
      'In review →to Done',
      'All statuses →to Cancelled',
    ]);
    expect(tab().querySelector('table.transitions td.move [aria-hidden="true"]')?.textContent).toBe('→');
    expect(await axeViolations(tab())).toEqual([]);
  });

  it('says what keeps a workflow from being published', async () => {
    const { tab, click, text } = await openWorkflows();
    await click(tab().querySelectorAll('.item')[1]);
    expect(text(tab().querySelector('#workflow-title'))).toBe('Bug workflow bug');
    expect(text(tab().querySelector('.problems-title'))).toBe('This workflow cannot be published yet:');
    expect(text(tab().querySelector('.problems li'))).toBe('nothing leads to “Fixed” from the initial status.');
    expect(tab().querySelector('.problems')?.getAttribute('role')).toBe('alert');
    expect(await axeViolations(tab())).toEqual([]);
  });

  it('adds a status with its category and portal label, and says when its identifier is taken', async () => {
    const { tab, dialog, reply, click, fill, choose, text, root } = await openWorkflows();
    await click(tab().querySelector('button.add-status'));
    expect(text(dialog().querySelector('h2'))).toBe('New status');
    await fill('input[lang="fr"]', 'En attente');
    await fill('input[lang="en"]', 'On hold');
    await fill('input[formcontrolname="key"]', 'in_review');
    await choose(0, 'In progress');
    const portal = dialog().querySelectorAll('ui-translatable-input')[1];
    (portal.querySelector('input[lang="en"]') as HTMLInputElement).value = 'Waiting for you';
    portal.querySelector('input[lang="en"]')!.dispatchEvent(new Event('input'));
    expect(await axeViolations(dialog())).toEqual([]);
    await click(dialog().querySelector('button.confirm'));
    const taken = await reply(
      (url, method) => url.endsWith('/workflows/w1/statuses') && method === 'POST',
      problem(409, 'workflow.key_taken', [{ field: 'key', code: 'taken' }]),
      409,
    );
    expect(taken.request.body).toEqual({
      key: 'in_review',
      labelTranslations: { fr: 'En attente', en: 'On hold' },
      portalLabelTranslations: { en: 'Waiting for you' },
      category: 'in_progress',
      isInitial: false,
    });
    expect(text(dialog().querySelector('mat-error'))).toBe('This value is already used.');

    await fill('input[formcontrolname="key"]', 'on_hold');
    await click(dialog().querySelector('button.confirm'));
    const saved = { ...STANDARD, statuses: [...STANDARD.statuses, status('s6', 'on_hold', 'En attente', 'On hold', 'in_progress')] };
    await reply((url, method) => url.endsWith('/workflows/w1/statuses') && method === 'POST', saved, 201);
    await reply((url) => url === `/api/v1/project-configs/${CONFIG.id}`, { ...CONFIG, draft: { number: 3, changeCount: 1, createdAt: '2026-11-13T08:00:00Z' } });
    expect(text(tab().querySelector('[role="status"]'))).toBe('Status saved in “Standard workflow”.');
    expect(tab().querySelectorAll('table.statuses tr[mat-row]').length).toBe(6);
    expect(text(root().querySelector('app-config-page app-config-chips'))).toContain('Unpublished draft · 1 change');
  });

  it('adds a transition from all statuses', async () => {
    const { tab, dialog, reply, click, fill, choose, text } = await openWorkflows();
    await click(tab().querySelector('button.add-transition'));
    expect(text(dialog().querySelector('h2'))).toBe('New transition');
    await fill('input[lang="fr"]', 'Bloquer');
    await fill('input[lang="en"]', 'Block');
    // "From" stays on all statuses.
    expect(text(dialog().querySelectorAll('mat-select')[0])).toBe('All statuses');
    await choose(1, 'In review');
    await click(dialog().querySelector('button.confirm'));
    const created = await reply((url, method) => url.endsWith('/workflows/w1/transitions') && method === 'POST', STANDARD, 201);
    expect(created.request.body).toEqual({
      nameTranslations: { fr: 'Bloquer', en: 'Block' },
      fromId: null,
      toId: 's3',
      // No rules yet (L04-US05), no screen (US06).
      rules: { screenId: null, conditions: { roleIds: [], groupIds: [] }, validators: [], postFunctions: [] },
    });
    expect(created.request.headers.get('Idempotency-Key')).toBeTruthy();
  });

  it('removes a status after a confirmation, and says why a workflow stays', async () => {
    const { tab, root, reply, click, text, waitFor } = await openWorkflows();
    const confirm = () => Array.from(root().querySelectorAll('mat-dialog-container button')).find((b) => text(b) === 'Remove')!;
    await click(tab().querySelectorAll('button.remove-status')[2]);
    expect(text(root().querySelector('mat-dialog-container h2'))).toBe('Remove the status “In review”?');
    await click(confirm());
    const removed = await reply((url, method) => url.endsWith('/workflow-statuses/s3') && method === 'DELETE', null, 204);
    expect(removed.request.headers.get('If-Match')).toBe('"1"');
    await reply((url) => url.endsWith('/workflows'), { stage: 'draft', items: [STANDARD, BUG] });
    await reply((url) => url.endsWith('/issue-types'), TYPES);
    await reply((url) => url === `/api/v1/project-configs/${CONFIG.id}`, CONFIG);
    expect(text(tab().querySelector('[role="status"]'))).toBe('Status “In review” removed from the draft.');

    await click(tab().querySelector('button.remove-workflow'));
    await waitFor(() => confirm() !== undefined);
    await click(confirm());
    await reply((url, method) => url.endsWith('/workflows/w1') && method === 'DELETE', problem(409, 'workflow.in_use'), 409);
    expect(text(tab().querySelector('[role="alert"]'))).toBe('Issue types follow this workflow: move them to another one first.');
  });
});
