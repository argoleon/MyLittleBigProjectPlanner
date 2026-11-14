/**
 * Tests of the rules of a transition (L04-US05): the form of each rule
 * type and what it sends, the values of "Set a field" by field type, the
 * order of the post-functions, the API errors placed on them, the
 * summaries of the transitions table; then the transition dialog in the
 * Workflows tab with the API mocked at the HTTP level and the real
 * translations, and accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import {
  ApiProblem,
  AuthService,
  ConfigField,
  ProjectConfig,
  TransitionRuleReferences,
  TransitionRules,
  Workflow,
  WorkflowStatus,
  provideApi,
  provideMaterialDefaults,
} from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { ConfigPage } from './config-page';
import {
  apiValue,
  conditionNames,
  formValue,
  movePostFunction,
  placeRuleErrors,
  postFunctionSummaries,
  rulesForm,
  rulesValue,
  valueInput,
} from './transition-rules';
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

const field = (id: string, en: string, kind: ConfigField['kind'], options: ConfigField['options'] = {}): ConfigField => ({
  id,
  key: id,
  label: en,
  labelTranslations: { en },
  kind,
  options,
  isSystem: false,
  allTypes: true,
  issueTypeIds: [],
  rules: [],
  version: 1,
});

const FIELDS: ConfigField[] = [
  field('f-remaining', 'Remaining work', 'number', { decimals: 2, unit: 'hours' }),
  field('f-app', 'Application', 'single_select', {
    values: [
      { id: 'v1', label: 'Directory', labelTranslations: { en: 'Directory' }, active: true, published: true, children: [] },
      { id: 'v2', label: 'Mail', labelTranslations: { en: 'Mail' }, active: false, published: true, children: [] },
    ],
  }),
  field('f-tags', 'Components', 'labels'),
  field('f-reviewer', 'Reviewer', 'user'),
];

const REFERENCES: TransitionRuleReferences = {
  roles: [{ id: 'r1', label: 'Reviewer', labelTranslations: { fr: 'Relecteur', en: 'Reviewer' } }],
  groups: [{ id: 'g1', name: 'Quality' }],
  members: [{ id: 'm1', name: 'Bob Martin' }],
};

/** "Validate" of mockup 11: reviewers or Quality, the remaining work required, then four post-functions. */
const VALIDATE_RULES: TransitionRules = {
  conditions: { roleIds: ['r1', 'r-gone'], groupIds: ['g1'] },
  validators: ['f-remaining'],
  postFunctions: [
    { kind: 'assign', assignTo: 'member', accountId: 'm1' },
    { kind: 'set_field', fieldId: 'f-tags', value: ['sso', 'web'] },
    { kind: 'clear_field', fieldId: 'f-remaining' },
    { kind: 'add_comment', text: 'Validated' },
  ],
};

describe('the rules of a transition', () => {
  it('reads and sends back every rule type', () => {
    const form = rulesForm(VALIDATE_RULES, REFERENCES, FIELDS);
    expect(form.controls.groups.value).toEqual([{ id: 'g1', name: 'Quality' }]);
    expect(form.controls.postFunctions.at(0).controls.member.value).toEqual({ id: 'm1', name: 'Bob Martin' });
    // Labels are edited as text.
    expect(form.controls.postFunctions.at(1).controls.value.value).toBe('sso, web');
    expect(form.valid).toBe(true);
    expect(rulesValue(form, FIELDS)).toEqual(VALIDATE_RULES);
  });

  it('sends only what each kind of post-function takes', () => {
    const form = rulesForm(null, null, FIELDS);
    expect(rulesValue(form, FIELDS)).toEqual({ conditions: { roleIds: [], groupIds: [] }, validators: [], postFunctions: [] });
    const assign = rulesForm({ postFunctions: [{ kind: 'assign', assignTo: 'reporter' }] }, null, FIELDS);
    // Changed to a comment: nothing of the assignment goes.
    const post = assign.controls.postFunctions.at(0);
    post.controls.kind.setValue('add_comment');
    post.controls.text.setValue('  Taken  ');
    expect(rulesValue(assign, FIELDS).postFunctions).toEqual([{ kind: 'add_comment', text: 'Taken' }]);
  });

  it('requires what each kind of post-function needs', () => {
    const form = rulesForm(
      {
        postFunctions: [
          { kind: 'assign', assignTo: 'member' },
          { kind: 'set_field' },
          { kind: 'clear_field' },
          { kind: 'add_comment', text: ' ' },
          { kind: 'assign', assignTo: 'nobody' },
        ],
      },
      null,
      FIELDS,
    );
    const errors = form.controls.postFunctions.controls.map((p) => Object.keys(p.errors ?? {}));
    expect(errors).toEqual([['member'], ['field'], ['field'], ['text'], []]);
  });

  it('enters the value of a field by its type', () => {
    expect(['number', 'single_select', 'labels', 'checkbox', 'user', 'date_time'].map((k) => valueInput(k as ConfigField['kind']))).toEqual([
      'number',
      'select',
      'labels',
      'checkbox',
      null,
      null,
    ]);
    expect(apiValue('labels', 'sso,  web ,')).toEqual(['sso', 'web']);
    expect(apiValue('number', '2.5')).toBe(2.5);
    expect(apiValue('number', '')).toBeNull();
    expect(apiValue('checkbox', null)).toBe(false);
    expect(apiValue('text', '  ')).toBeNull();
    expect(apiValue('multi', null)).toEqual([]);
    expect(formValue('checkbox', undefined)).toBe(false);
    expect(formValue('multi', null)).toEqual([]);
    expect(formValue('labels', ['a', 'b'])).toBe('a, b');
  });

  it('runs the post-functions in the order they are moved to', () => {
    const form = rulesForm(VALIDATE_RULES, REFERENCES, FIELDS);
    movePostFunction(form.controls.postFunctions, 3, 0);
    movePostFunction(form.controls.postFunctions, 0, -1);
    expect(rulesValue(form, FIELDS).postFunctions?.map((p) => p.kind)).toEqual(['add_comment', 'assign', 'set_field', 'clear_field']);
  });

  it('puts the errors of the API on their post-function', () => {
    const form = rulesForm(VALIDATE_RULES, REFERENCES, FIELDS);
    const invalid = new ApiProblem(400, 'validation_failed', 'invalid', null, [
      { field: 'rules.postFunctions[0].accountId', code: 'unknown_member' },
      { field: 'rules.postFunctions[1].value', code: 'invalid_label' },
    ]);
    expect(placeRuleErrors(invalid, form)).toBe(true);
    expect(form.controls.postFunctions.at(0).controls.member.getError('api')).toBe('errors.field.unknown_member');
    expect(form.controls.postFunctions.at(1).controls.value.getError('api')).toBe('errors.field.invalid_label');
    // A role deleted since: nothing holds it, the dialog says it above.
    const role = new ApiProblem(400, 'validation_failed', 'invalid', null, [{ field: 'rules.conditions.roleIds[0]', code: 'not_found' }]);
    expect(placeRuleErrors(role, form)).toBe(false);
  });

  it('sums up who may take a transition and what it does', () => {
    const names = conditionNames(VALIDATE_RULES, REFERENCES, (labels) => labels['en'] ?? '');
    expect(names).toEqual({ roles: ['Reviewer', null], groups: ['Quality'] });
    const fieldName = (id: string | null | undefined) => FIELDS.find((f) => f.id === id)?.label ?? '';
    expect(postFunctionSummaries(VALIDATE_RULES, REFERENCES, fieldName)).toEqual([
      { key: 'config.workflows.rules.summary.assignMember', params: { name: 'Bob Martin' } },
      { key: 'config.workflows.rules.summary.set', params: { field: 'Components' } },
      { key: 'config.workflows.rules.summary.clear', params: { field: 'Remaining work' } },
      { key: 'config.workflows.rules.summary.comment', params: {} },
    ]);
    expect(postFunctionSummaries({ postFunctions: [{ kind: 'assign', assignTo: 'current_user' }] }, null, fieldName)[0].key).toBe(
      'config.workflows.rules.summary.assign.current_user',
    );
  });
});

const status = (id: string, en: string): WorkflowStatus => ({
  id,
  key: id,
  label: en,
  labelTranslations: { en },
  portalLabel: null,
  portalLabelTranslations: {},
  category: 'in_progress',
  isInitial: id === 's1',
  version: 1,
});

const NO_RULES: TransitionRules = { conditions: { roleIds: [], groupIds: [] }, validators: [], postFunctions: [] };

const STANDARD: Workflow = {
  id: 'w1',
  key: 'standard',
  label: 'Standard workflow',
  labelTranslations: { en: 'Standard workflow' },
  issueTypeIds: [],
  statuses: [status('s1', 'To do'), status('s2', 'In review'), status('s3', 'Done')],
  transitions: [
    {
      id: 'x1',
      name: 'Send to review',
      nameTranslations: { fr: 'Passer en revue', en: 'Send to review' },
      fromId: 's1',
      toId: 's2',
      rules: NO_RULES,
      version: 1,
    },
    { id: 'x2', name: 'Validate', nameTranslations: { fr: 'Valider', en: 'Validate' }, fromId: 's2', toId: 's3', rules: VALIDATE_RULES, version: 3 },
  ],
  problems: [],
  version: 1,
};

const SESSION = {
  account: { id: 'a1', email: 'lucas@acme.example', displayName: 'Lucas Bernard', locale: null, tenantRole: 'configuration_admin' },
  stage: 'complete',
  csrfToken: 'csrf',
  expiresAt: '2026-11-20T09:00:00Z',
};

describe('the transition dialog with its rules', () => {
  async function openWorkflows() {
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
    await reply((url) => url.endsWith('/workflows'), { stage: 'published', items: [STANDARD], references: REFERENCES });
    await reply((url) => url.endsWith('/issue-types'), { stage: 'published', items: [] });
    await reply((url) => url.endsWith('/fields'), { stage: 'published', items: FIELDS });
    const root = () => document.body;
    const tab = () => root().querySelector('app-workflows-tab')!;
    const dialog = () => root().querySelector('mat-dialog-container')!;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const texts = (selector: string, within: Element = tab()) => Array.from(within.querySelectorAll(selector)).map((e) => text(e));
    const click = async (element: Element | null | undefined) => {
      (element as HTMLElement).click();
      await settle();
    };
    const type = async (input: Element | null, value: string) => {
      (input as HTMLInputElement).focus();
      input!.dispatchEvent(new Event('focusin'));
      (input as HTMLInputElement).value = value;
      input!.dispatchEvent(new Event('input'));
      await settle();
    };
    const option = (label: string) => Array.from(root().querySelectorAll('mat-option')).find((o) => text(o)?.startsWith(label));
    return { reply, root, tab, dialog, text, texts, click, type, option };
  }

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it('sums up the rules of each transition in the table', async () => {
    const { texts } = await openWorkflows();
    expect(texts('table.transitions td.rules')).toEqual([
      'Open to everyone',
      'lock Only: Reviewer, deleted, Quality rule Requires Remaining work bolt Assigns to Bob Martin bolt Sets Components bolt Clears Remaining work bolt Adds a comment',
    ]);
  });

  it('edits the rules of a transition and sends them in its body', async () => {
    const { tab, dialog, root, reply, click, type, option, text, texts } = await openWorkflows();
    await click(tab().querySelectorAll('button.edit-transition')[0]);
    await reply((url) => url.endsWith('/transition-roles'), REFERENCES.roles);
    expect(texts('app-transition-rules legend', dialog())).toEqual(['Who may take it', 'Required fields', 'Post-functions']);

    // Reviewers, or the quality group found by name.
    await click(dialog().querySelectorAll('app-transition-rules mat-select .mat-mdc-select-trigger')[0]);
    await click(option('Reviewer'));
    // The roles stay open for more: closed before searching a group.
    await click(Array.from(root().querySelectorAll('.cdk-overlay-backdrop')).at(-1));
    await type(dialog().querySelector('input.group-search'), 'qual');
    await reply((url) => url.startsWith('/api/v1/project-configs/transition-candidates') && url.includes('search=qual'), [
      { kind: 'group', id: 'g1', name: 'Quality', email: null },
      { kind: 'member', id: 'm2', name: 'Quality Bot', email: 'bot@acme.example' },
    ]);
    // Groups only: the member found is left out.
    const found = Array.from(root().querySelectorAll('.mat-mdc-autocomplete-panel mat-option'));
    expect(found.map((o) => text(o))).toEqual(['Quality']);
    await click(found[0]);
    expect(texts('app-transition-rules mat-chip', dialog())).toEqual(['Quality cancel']);

    // Assign to whoever takes it, then a comment.
    await click(dialog().querySelector('button.add-post'));
    await click(dialog().querySelector('button.add-post'));
    const kinds = () => dialog().querySelectorAll('li.post mat-select');
    await click(kinds()[2].querySelector('.mat-mdc-select-trigger'));
    await click(option('Add a comment'));
    await type(dialog().querySelector('li.post:nth-child(2) textarea'), 'Sent to review');
    expect(await axeViolations(dialog())).toEqual([]);

    await click(dialog().querySelector('button.confirm'));
    const saved = await reply((url, method) => url.endsWith('/workflow-transitions/x1') && method === 'PUT', STANDARD);
    expect(saved.request.headers.get('If-Match')).toBe('"1"');
    expect(saved.request.body).toEqual({
      nameTranslations: { fr: 'Passer en revue', en: 'Send to review' },
      fromId: 's1',
      toId: 's2',
      rules: {
        conditions: { roleIds: ['r1'], groupIds: ['g1'] },
        validators: [],
        postFunctions: [
          { kind: 'assign', assignTo: 'current_user' },
          { kind: 'add_comment', text: 'Sent to review' },
        ],
      },
    });
  });

  it('shows the error of the API on the post-function it names', async () => {
    const { tab, dialog, reply, click, text } = await openWorkflows();
    await click(tab().querySelectorAll('button.edit-transition')[1]);
    await reply((url) => url.endsWith('/transition-roles'), REFERENCES.roles);
    // The existing rules, with the names of what they refer to.
    expect(text(dialog().querySelector('mat-chip'))).toBe('Quality cancel');
    expect((dialog().querySelector('input.member-search') as HTMLInputElement).value).toBe('Bob Martin');
    expect((dialog().querySelector('li.post:nth-child(2) input.value') as HTMLInputElement).value).toBe('sso, web');
    expect(await axeViolations(dialog())).toEqual([]);
    await click(dialog().querySelector('button.confirm'));
    await reply(
      (url, method) => url.endsWith('/workflow-transitions/x2') && method === 'PUT',
      {
        type: 'urn:mlbpp:problem:validation_failed',
        title: 'invalid',
        status: 400,
        code: 'validation_failed',
        errors: [{ field: 'rules.postFunctions[0].accountId', code: 'unknown_member' }],
      },
      400,
    );
    expect(text(dialog().querySelector('li.post .error-hint'))).toBe('Not a member of the organization.');
    expect(dialog().querySelector('.message.error')).toBeNull();
  });
});
