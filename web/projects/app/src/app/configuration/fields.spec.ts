/**
 * Tests of the Fields tab of a configuration (L04-US03): the rules grid
 * helpers (cycling, one grid per workflow), the list values editor
 * (deactivated once published, removed otherwise, reordered), then the tab
 * in the configuration page with the API mocked at the HTTP level and the
 * real translations: the fields table, the panel of a custom field, a new
 * field, a system field, and accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import {
  AuthService,
  ConfigField,
  FieldValue,
  IssueType,
  ProjectConfig,
  Workflow,
  WorkflowStatus,
  provideApi,
  provideMaterialDefaults,
} from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { ConfigPage } from './config-page';
import { moveValue, removeValue, valuesForm, valuesInput } from './field-values';
import { cycled, languagesOf, modeOf, nextMode, rulesFor, rulesGrids, typesOf, withMode } from './fields';
import { FieldsTab } from './fields-tab';

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

const status = (id: string, fr: string, en: string): WorkflowStatus => ({
  id,
  key: id,
  label: en,
  labelTranslations: { fr, en },
  portalLabel: null,
  portalLabelTranslations: {},
  category: 'in_progress',
  isInitial: false,
  version: 1,
});

const workflow = (id: string, en: string, statuses: WorkflowStatus[]): Workflow => ({
  id,
  key: id,
  label: en,
  labelTranslations: { en },
  issueTypeIds: [],
  statuses,
  transitions: [],
  problems: [],
  version: 1,
});

/** The standard workflow (shortened), and one of its own for bugs. */
const STANDARD = workflow('w1', 'Standard workflow', [
  status('s1', 'À faire', 'To do'),
  status('s2', 'En cours', 'In progress'),
  status('s3', 'Terminé', 'Done'),
]);
const BUGS = workflow('w2', 'Bug workflow', [status('b1', 'Ouvert', 'Open'), status('b2', 'Corrigé', 'Fixed')]);

const type = (id: string, en: string, fr: string, workflowId: string): IssueType => ({
  id,
  key: en.toLowerCase(),
  label: en,
  labelTranslations: { fr, en },
  icon: 'task_alt',
  color: 'task',
  isSubtask: false,
  parentIds: [],
  workflowId,
  level: 1,
  version: 1,
});

const TYPES = [type('t2', 'Story', 'Story', 'w1'), type('t3', 'Task', 'Tâche', 'w1'), type('t4', 'Bug', 'Bug', 'w2')];

const value = (id: string, fr: string, en: string, published: boolean, children: FieldValue[] = []): FieldValue => ({
  id,
  label: en,
  labelTranslations: { fr, en },
  active: true,
  published,
  children,
});

/** Mockup 12: Application on Story, Task and Bug; Bug required at creation, Task hidden there. */
const APPLICATION: ConfigField = {
  id: 'f1',
  key: 'application',
  label: 'Application',
  labelTranslations: { fr: 'Application', en: 'Application' },
  kind: 'single_select',
  options: {
    values: [value('v1', 'Annuaire', 'Directory', true), value('v2', 'Messagerie', 'Mail', true), value('v3', 'ERP', 'ERP', false)],
  },
  isSystem: false,
  allTypes: false,
  issueTypeIds: ['t2', 't3', 't4'],
  rules: [
    { issueTypeId: 't3', statusId: null, mode: 'hidden' },
    { issueTypeId: 't4', statusId: null, mode: 'required' },
  ],
  version: 2,
};

const POINTS: ConfigField = {
  id: 'f2',
  key: 'story_points',
  label: 'Points',
  labelTranslations: { fr: 'Points', en: 'Points' },
  kind: 'number',
  options: { decimals: 1 },
  isSystem: true,
  allTypes: false,
  issueTypeIds: ['t2', 't3'],
  rules: [],
  version: 1,
};

const REMAINING: ConfigField = {
  id: 'f3',
  key: 'remaining',
  label: 'Remaining work',
  labelTranslations: { fr: 'Reste à faire', en: 'Remaining work' },
  kind: 'number',
  options: { decimals: 2, unit: 'hours' },
  isSystem: true,
  allTypes: true,
  issueTypeIds: [],
  rules: [],
  version: 1,
};

const ORIGIN: ConfigField = {
  ...APPLICATION,
  id: 'f4',
  key: 'origin_request',
  label: 'Ticket d’origine',
  labelTranslations: { fr: 'Ticket d’origine' },
  kind: 'issue_link',
  options: {},
  issueTypeIds: ['t4'],
  rules: [],
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

describe('the rules of a field', () => {
  it('cycles a cell from Hidden to Visible, then Required, and back', () => {
    expect(nextMode('hidden')).toBe('visible');
    expect(nextMode('visible')).toBe('required');
    expect(nextMode('required')).toBe('hidden');
  });

  it('keeps only the cells that are not Visible', () => {
    const rules = APPLICATION.rules;
    expect(modeOf(rules, 't3', null)).toBe('hidden');
    expect(modeOf(rules, 't2', 's2')).toBe('visible');
    // Task at creation: Hidden > Visible, then the cell is no longer stored.
    const visible = cycled(rules, 't3', null);
    expect(visible).toEqual([{ issueTypeId: 't4', statusId: null, mode: 'required' }]);
    expect(cycled(visible, 't3', null)).toContainEqual({ issueTypeId: 't3', statusId: null, mode: 'required' });
    // A status cell, apart from the creation.
    expect(withMode(rules, 't4', 'b2', 'required')).toHaveLength(3);
    expect(withMode(rules, 't4', null, 'visible')).toEqual([{ issueTypeId: 't3', statusId: null, mode: 'hidden' }]);
  });

  it('makes one grid per workflow of the types the field applies to', () => {
    const grids = rulesGrids(TYPES, [STANDARD, BUGS], APPLICATION);
    expect(grids.map((g) => [g.workflow.id, g.types.map((t) => t.id), g.statuses.length])).toEqual([
      ['w1', ['t2', 't3'], 3],
      ['w2', ['t4'], 2],
    ]);
    expect(rulesGrids(TYPES, [STANDARD, BUGS], { allTypes: false, issueTypeIds: ['t4'] }).map((g) => g.workflow.id)).toEqual(['w2']);
    expect(rulesGrids(TYPES, [STANDARD, BUGS], { allTypes: true, issueTypeIds: [] })).toHaveLength(2);
    // Rules of a type the field no longer applies to go.
    expect(rulesFor(APPLICATION.rules, { allTypes: false, issueTypeIds: ['t4'] })).toEqual([
      { issueTypeId: 't4', statusId: null, mode: 'required' },
    ]);
  });

  it('names the types and the languages of a field', () => {
    expect(typesOf(APPLICATION, TYPES)?.map((t) => t.id)).toEqual(['t2', 't3', 't4']);
    expect(typesOf(REMAINING, TYPES)).toBeNull();
    expect(languagesOf(APPLICATION)).toEqual(['FR', 'EN']);
    expect(languagesOf(ORIGIN)).toEqual(['FR']);
  });
});

describe('the list values of a field', () => {
  it('deactivates a published value and removes one added since', () => {
    const values = valuesForm(APPLICATION.options.values);
    removeValue(values, 0);
    expect(values.length).toBe(3);
    expect(values.at(0).controls.active.value).toBe(false);
    removeValue(values, 2);
    expect(values.length).toBe(2);
    expect(valuesInput(values, false)).toEqual([
      { id: 'v1', labelTranslations: { fr: 'Annuaire', en: 'Directory' }, active: false, children: [] },
      { id: 'v2', labelTranslations: { fr: 'Messagerie', en: 'Mail' }, active: true, children: [] },
    ]);
  });

  it('reorders the values, and sends the ones under a value only for a cascade', () => {
    const values = valuesForm([value('v1', 'ERP', 'ERP', true, [value('c1', 'Finance', 'Finance', true)]), value('v2', 'Mail', 'Mail', false)]);
    moveValue(values, 1, 0);
    expect(values.controls.map((v) => v.controls.id.value)).toEqual(['v2', 'v1']);
    moveValue(values, 0, 5);
    expect(values.controls.map((v) => v.controls.id.value)).toEqual(['v2', 'v1']);
    expect(valuesInput(values, true)[1].children).toEqual([
      { id: 'c1', labelTranslations: { fr: 'Finance', en: 'Finance' }, active: true, children: [] },
    ]);
    expect(valuesInput(values, false)[1].children).toEqual([]);
    // A new value has no identity yet.
    values.push(valuesForm([{ ...value('x', 'Neuf', 'New', false), id: '' }]).at(0));
    expect(valuesInput(values, false)[2]).toEqual({ labelTranslations: { fr: 'Neuf', en: 'New' }, active: true, children: [] });
  });
});

describe('FieldsTab', () => {
  async function openFields(items: ConfigField[] = [APPLICATION, POINTS, REMAINING, ORIGIN]) {
    localStorage.clear();
    TestBed.configureTestingModule({
      providers: [
        provideRouter(
          [{ path: 'admin/configurations/:id', component: ConfigPage, children: [{ path: 'fields', component: FieldsTab }] }],
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
    await harness.navigateByUrl(`/admin/configurations/${CONFIG.id}/fields`);
    await reply((url) => url === `/api/v1/project-configs/${CONFIG.id}`, CONFIG);
    await reply((url, method) => url.endsWith('/fields') && method === 'GET', { stage: 'published', items });
    await reply((url) => url.endsWith('/issue-types'), { stage: 'published', items: TYPES });
    await reply((url) => url.endsWith('/workflows'), { stage: 'published', items: [STANDARD, BUGS] });
    const root = () => document.body;
    const tab = () => root().querySelector('app-fields-tab')!;
    const panel = () => tab().querySelector('app-field-editor')!;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const texts = (selector: string, within: Element = tab()) => Array.from(within.querySelectorAll(selector)).map((e) => text(e));
    const click = async (element: Element | null | undefined) => {
      (element as HTMLElement).click();
      await settle();
    };
    const fill = async (input: Element | null, value: string) => {
      (input as HTMLInputElement).value = value;
      input!.dispatchEvent(new Event('input'));
      input!.dispatchEvent(new Event('blur'));
      await settle();
    };
    const cell = (label: string) => panel().querySelector(`app-rules-grid button[aria-label="${label}"]`);
    const option = (label: string) => Array.from(root().querySelectorAll('mat-option')).find((o) => text(o)?.endsWith(label));
    return { reply, root, tab, panel, text, texts, click, fill, cell, option };
  }

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it('lists the fields with their type, issue types and languages, system ones marked', async () => {
    const { tab, text, texts } = await openFields();
    const rows = Array.from(tab().querySelectorAll('tr[mat-row]'));
    expect(rows.map((row) => text(row.querySelector('.label')))).toEqual(['Application', 'Points', 'Remaining work', 'Ticket d’origine']);
    expect(texts('tr[mat-row] .system')).toEqual(['System', 'System']);
    expect(texts('tr[mat-row] code')).toEqual(['application', 'story_points', 'remaining', 'origin_request']);
    expect(texts('tr[mat-row] .mat-column-kind')).toEqual(['Single select', 'Number', 'Number Hours', 'Link to a ticket']);
    expect(texts('tr[mat-row] .mat-column-types')).toEqual(['Story, Task, Bug', 'Story, Task', 'All', 'Bug']);
    expect(texts('tr[mat-row] .mat-column-languages')).toEqual(['FR, EN', 'FR, EN', 'FR, EN', 'FR']);
    expect(rows[0].querySelector('mat-icon')?.textContent).toBe('list');
    expect(await axeViolations(tab())).toEqual([]);
  });

  it('edits the values and cycles the rules of a custom field, one grid per workflow', async () => {
    const { tab, panel, text, texts, click, fill, cell, reply, root } = await openFields();
    await click(tab().querySelector('button.edit'));
    expect(text(panel().querySelector('h2'))).toBe('Edit “Application”');
    expect(texts('app-rules-grid caption', panel())).toEqual(['Types following Standard workflow', 'Types following Bug workflow']);
    expect(texts('app-rules-grid tbody th', panel())).toEqual(['Story', 'Task', 'Bug']);
    // A cell per type and step, with its name: Creation, then each status.
    expect(panel().querySelectorAll('app-rules-grid button.mode').length).toBe(2 * 4 + 3);
    expect(text(cell('Task, Creation: Hidden'))).toBe('Hidden');
    expect(cell('Bug, Creation: Required')?.getAttribute('data-mode')).toBe('required');
    await click(cell('Task, Creation: Hidden'));
    expect(cell('Task, Creation: Visible')).not.toBeNull();
    await click(cell('Story, Done: Visible'));
    expect(cell('Story, Done: Required')).not.toBeNull();
    // Directory was published: deactivated; ERP was not: removed.
    expect(panel().querySelectorAll('.value-list > li').length).toBe(3);
    await click(panel().querySelectorAll('button.value-menu')[0]);
    await click(Array.from(root().querySelectorAll('[mat-menu-item]')).find((b) => text(b)?.includes('Deactivate')));
    expect(text(panel().querySelector('.value.inactive .state'))).toBe('Deactivated: no longer offered, kept on the tickets that have it.');
    await click(panel().querySelectorAll('button.value-menu')[2]);
    await click(Array.from(root().querySelectorAll('[mat-menu-item]')).find((b) => text(b)?.includes('Remove')));
    expect(panel().querySelectorAll('.value-list > li').length).toBe(2);
    await fill(panel().querySelector('input[aria-label="Value 2 (English)"]'), 'Email');
    expect(await axeViolations(panel())).toEqual([]);

    await click(panel().querySelector('button.save'));
    const saved = await reply((url, method) => url.endsWith('/fields/f1') && method === 'PUT', { ...APPLICATION, version: 3 });
    expect(saved.request.headers.get('If-Match')).toBe('"2"');
    expect(saved.request.body).toEqual({
      labelTranslations: { fr: 'Application', en: 'Application' },
      options: {
        values: [
          { id: 'v1', labelTranslations: { fr: 'Annuaire', en: 'Directory' }, active: false, children: [] },
          { id: 'v2', labelTranslations: { fr: 'Messagerie', en: 'Email' }, active: true, children: [] },
        ],
      },
      allTypes: false,
      issueTypeIds: ['t2', 't3', 't4'],
      rules: [
        { issueTypeId: 't4', statusId: null, mode: 'required' },
        { issueTypeId: 't2', statusId: 's3', mode: 'required' },
      ],
    });
    await reply((url, method) => url.endsWith('/fields') && method === 'GET', { stage: 'draft', items: [APPLICATION] });
    await reply((url) => url === `/api/v1/project-configs/${CONFIG.id}`, CONFIG);
    expect(text(tab().querySelector('[role="status"]'))).toBe('Field “Application” saved in the draft.');
  });

  it('creates a single select with its values, issue types and rules, and says when its identifier is taken', async () => {
    const { tab, panel, text, click, fill, cell, option, reply } = await openFields();
    await click(tab().querySelector('button.add'));
    expect(text(panel().querySelector('h2'))).toBe('New field');
    expect(text(panel().querySelector('app-rules-grid .empty'))).toBe('Choose the issue types of the field to set its rules.');
    await fill(panel().querySelector('ui-translatable-input input[lang="fr"]'), 'Environnement');
    await fill(panel().querySelector('ui-translatable-input input[lang="en"]'), 'Environment');
    await fill(panel().querySelector('input[formcontrolname="key"]'), 'application');
    await click(panel().querySelectorAll('mat-select .mat-mdc-select-trigger')[0]);
    await click(option('Single select'));
    // A list starts with one value to fill in.
    await fill(panel().querySelector('input[aria-label="Value 1 (Français)"]'), 'Production');
    await fill(panel().querySelector('input[aria-label="Value 1 (English)"]'), 'Production');
    await click(panel().querySelector('button.add-value'));
    await fill(panel().querySelector('input[aria-label="Value 2 (Français)"]'), 'Recette');
    await fill(panel().querySelector('input[aria-label="Value 2 (English)"]'), 'Staging');
    const selects = panel().querySelectorAll('mat-select .mat-mdc-select-trigger');
    await click(selects[selects.length - 1]);
    await click(option('Bug'));
    await click(cell('Bug, Creation: Visible'));
    expect(cell('Bug, Creation: Required')).not.toBeNull();
    expect(await axeViolations(panel())).toEqual([]);

    await click(panel().querySelector('button.save'));
    const taken = await reply(
      (url, method) => url.endsWith('/fields') && method === 'POST',
      problem(409, 'config_field.key_taken', [{ field: 'key', code: 'taken' }]),
      409,
    );
    expect(taken.request.headers.get('Idempotency-Key')).toBeTruthy();
    expect(taken.request.body).toEqual({
      key: 'application',
      kind: 'single_select',
      labelTranslations: { fr: 'Environnement', en: 'Environment' },
      options: {
        values: [
          { labelTranslations: { fr: 'Production', en: 'Production' }, active: true, children: [] },
          { labelTranslations: { fr: 'Recette', en: 'Staging' }, active: true, children: [] },
        ],
      },
      allTypes: false,
      issueTypeIds: ['t4'],
      rules: [{ issueTypeId: 't4', statusId: null, mode: 'required' }],
    });
    expect(text(panel().querySelector('mat-error'))).toBe('This value is already used.');
  });

  it('keeps the label and options of a system field, and changes its types and rules', async () => {
    const { tab, panel, text, click, cell, reply } = await openFields();
    await click(tab().querySelectorAll('button.edit')[1]);
    expect(text(panel().querySelector('h2'))).toBe('Edit “Points”');
    expect(text(panel().querySelector('.hint.system'))).toBe(
      'A system field keeps its label and options; you choose its issue types and rules.',
    );
    expect((panel().querySelector('ui-translatable-input input[lang="en"]') as HTMLInputElement).disabled).toBe(true);
    expect((panel().querySelector('input[formcontrolname="key"]') as HTMLInputElement).disabled).toBe(true);
    expect(panel().querySelector('button.remove')).toBeNull();
    await click(cell('Story, In progress: Visible'));
    await click(cell('Story, In progress: Required'));
    expect(cell('Story, In progress: Hidden')).not.toBeNull();
    expect(await axeViolations(panel())).toEqual([]);
    await click(panel().querySelector('button.save'));
    const saved = await reply((url, method) => url.endsWith('/fields/f2') && method === 'PUT', POINTS);
    expect(saved.request.body).toEqual({
      labelTranslations: { fr: 'Points', en: 'Points' },
      options: { decimals: 1 },
      allTypes: false,
      issueTypeIds: ['t2', 't3'],
      rules: [{ issueTypeId: 't2', statusId: 's2', mode: 'hidden' }],
    });
  });

  it('removes a custom field after a confirmation', async () => {
    const { tab, panel, root, click, text, reply } = await openFields();
    await click(tab().querySelectorAll('button.edit')[3]);
    await click(panel().querySelector('button.remove'));
    expect(text(root().querySelector('mat-dialog-container h2'))).toBe('Remove “Ticket d’origine”?');
    await click(Array.from(root().querySelectorAll('mat-dialog-container button')).find((b) => text(b) === 'Remove'));
    const removed = await reply((url, method) => url.endsWith('/fields/f4') && method === 'DELETE', null, 204);
    expect(removed.request.headers.get('If-Match')).toBe('"2"');
    await reply((url, method) => url.endsWith('/fields') && method === 'GET', { stage: 'draft', items: [APPLICATION] });
    await reply((url) => url === `/api/v1/project-configs/${CONFIG.id}`, CONFIG);
    expect(text(tab().querySelector('[role="status"]'))).toBe('Field “Ticket d’origine” removed from the draft.');
    expect(tab().querySelector('app-field-editor')).toBeNull();
  });
});
