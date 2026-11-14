/**
 * Tests of the Screens tab of a configuration (L04-US06): adding,
 * removing and reordering fields, the preview where the rules win, the
 * required fields a screen lacks; then the tab in the configuration page
 * with the API mocked at the HTTP level and the real translations: the
 * screens by kind, the editor with the keyboard, a new screen for a type,
 * a transition screen kept while a transition shows it, and accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import {
  AuthService,
  ConfigField,
  ConfigScreen,
  IssueType,
  ProjectConfig,
  Workflow,
  WorkflowStatus,
  provideApi,
  provideMaterialDefaults,
} from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { ConfigPage } from './config-page';
import { available, missingRequired, moved, preview, stepsOf, typesOf, withField, withoutField } from './screens';
import { ScreensTab } from './screens-tab';

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

const WORKFLOW: Workflow = {
  id: 'w1',
  key: 'standard',
  label: 'Standard workflow',
  labelTranslations: { en: 'Standard workflow' },
  issueTypeIds: ['t2', 't4'],
  statuses: [status('s1', 'To do'), status('s2', 'In progress'), status('s3', 'Done')],
  transitions: [],
  problems: [],
  version: 1,
};

const type = (id: string, en: string): IssueType => ({
  id,
  key: en.toLowerCase(),
  label: en,
  labelTranslations: { en },
  icon: 'task_alt',
  color: 'task',
  isSubtask: false,
  parentIds: [],
  workflowId: 'w1',
  level: 1,
  version: 1,
});

const TYPES = [type('t2', 'Story'), type('t4', 'Bug')];

const field = (id: string, en: string, rest: Partial<ConfigField> = {}): ConfigField => ({
  id,
  key: id,
  label: en,
  labelTranslations: { en },
  kind: 'short_text',
  options: {},
  isSystem: false,
  allTypes: true,
  issueTypeIds: [],
  rules: [],
  version: 1,
  ...rest,
});

/** Points for stories only; Application hidden for bugs at creation; the environment required for bugs in progress. */
const FIELDS: ConfigField[] = [
  field('f-points', 'Points', { kind: 'number', allTypes: false, issueTypeIds: ['t2'], isSystem: true }),
  field('f-app', 'Application', { rules: [{ issueTypeId: 't4', statusId: null, mode: 'hidden' }] }),
  field('f-env', 'Environment', { rules: [{ issueTypeId: 't4', statusId: 's2', mode: 'required' }] }),
  field('f-reason', 'Rejection reason'),
];

const screen = (id: string, rest: Partial<ConfigScreen>): ConfigScreen => ({
  id,
  kind: 'creation',
  issueTypeId: null,
  isGeneral: false,
  name: id,
  nameTranslations: { en: id },
  fieldIds: [],
  usedBy: [],
  version: 1,
  ...rest,
});

const SCREENS: ConfigScreen[] = [
  screen('creation', { isGeneral: true, name: 'Creation', nameTranslations: { en: 'Creation' }, fieldIds: ['f-points', 'f-app', 'f-env'] }),
  screen('bug-creation', { issueTypeId: 't4', name: 'Bug creation', nameTranslations: { en: 'Bug creation' }, fieldIds: ['f-app'] }),
  screen('view', {
    kind: 'view',
    isGeneral: true,
    name: 'View',
    nameTranslations: { fr: 'Consultation', en: 'View' },
    fieldIds: ['f-points', 'f-app'],
  }),
  screen('reason', {
    kind: 'transition',
    name: 'Rejection reason',
    nameTranslations: { en: 'Rejection reason' },
    fieldIds: ['f-reason'],
    usedBy: [{ transitionId: 'x3', workflowId: 'w1', transition: 'Reject', workflow: 'Standard workflow' }],
  }),
];

const SESSION = {
  account: { id: 'a1', email: 'lucas@acme.example', displayName: 'Lucas Bernard', locale: null, tenantRole: 'configuration_admin' },
  stage: 'complete',
  csrfToken: 'csrf',
  expiresAt: '2026-11-20T09:00:00Z',
};

describe('the fields of a screen', () => {
  it('are added, removed and reordered', () => {
    expect(withField(['a', 'b'], 'c')).toEqual(['a', 'b', 'c']);
    expect(withField(['a', 'b'], 'c', 0)).toEqual(['c', 'a', 'b']);
    expect(withField(['a', 'b'], 'a')).toEqual(['a', 'b']);
    expect(withoutField(['a', 'b'], 'a')).toEqual(['b']);
    expect(moved(['a', 'b', 'c'], 2, 0)).toEqual(['c', 'a', 'b']);
    expect(moved(['a', 'b'], 0, -1)).toEqual(['a', 'b']);
    expect(available(['f-app'], FIELDS).map((f) => f.id)).toEqual(['f-points', 'f-env', 'f-reason']);
  });

  it('show what the rules let through, the required ones added at the end', () => {
    const lines = (typeId: string, statusId: string | null) =>
      preview(['f-points', 'f-app', 'f-reason'], FIELDS, typeId, statusId).map((l) => [l.field.id, l.mode, l.added]);
    expect(lines('t2', null)).toEqual([
      ['f-points', 'visible', false],
      ['f-app', 'visible', false],
      ['f-reason', 'visible', false],
    ]);
    // A bug in progress: Points is not a bug's, the environment required and added.
    expect(lines('t4', 's2')).toEqual([
      ['f-points', 'hidden', false],
      ['f-app', 'visible', false],
      ['f-reason', 'visible', false],
      ['f-env', 'required', true],
    ]);
    expect(lines('t4', null)[1]).toEqual(['f-app', 'hidden', false]);
  });

  it('name the required fields a screen lacks where it is shown', () => {
    const view = { kind: 'view' as const, issueTypeId: null };
    expect(missingRequired(view, ['f-points'], FIELDS, TYPES, [WORKFLOW]).map((f) => f.id)).toEqual(['f-env']);
    // At creation nothing is required.
    expect(missingRequired({ kind: 'creation', issueTypeId: null }, [], FIELDS, TYPES, [WORKFLOW])).toEqual([]);
    // For stories only: the environment is required for bugs.
    expect(missingRequired({ kind: 'view', issueTypeId: 't2' }, [], FIELDS, TYPES, [WORKFLOW])).toEqual([]);
    expect(typesOf({ issueTypeId: 't4' }, TYPES).map((t) => t.id)).toEqual(['t4']);
    expect(stepsOf('creation', TYPES[0], [WORKFLOW])).toEqual([null]);
    expect(stepsOf('transition', TYPES[0], [WORKFLOW])).toEqual(['s1', 's2', 's3']);
  });
});

describe('ScreensTab', () => {
  async function openScreens() {
    localStorage.clear();
    TestBed.configureTestingModule({
      providers: [
        provideRouter(
          [{ path: 'admin/configurations/:id', component: ConfigPage, children: [{ path: 'screens', component: ScreensTab }] }],
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
    await harness.navigateByUrl(`/admin/configurations/${CONFIG.id}/screens`);
    await reply((url) => url === `/api/v1/project-configs/${CONFIG.id}`, CONFIG);
    await reply((url, method) => url.endsWith('/screens') && method === 'GET', { stage: 'published', items: SCREENS });
    await reply((url) => url.endsWith('/fields'), { stage: 'published', items: FIELDS });
    await reply((url) => url.endsWith('/issue-types'), { stage: 'published', items: TYPES });
    await reply((url) => url.endsWith('/workflows'), {
      stage: 'published',
      items: [WORKFLOW],
      references: { roles: [], groups: [], members: [] },
    });
    const root = () => document.body;
    const tab = () => root().querySelector('app-screens-tab')!;
    const panel = () => tab().querySelector('app-screen-editor')!;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const texts = (selector: string, within: Element = tab()) => Array.from(within.querySelectorAll(selector)).map((e) => text(e));
    const click = async (element: Element | null | undefined) => {
      (element as HTMLElement).click();
      await settle();
    };
    const button = (label: string) => panel().querySelector(`button[aria-label="${label}"]`);
    return { reply, root, tab, panel, text, texts, click, button };
  }

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it('lists the screens by kind, with the type each is for or the transitions that show it', async () => {
    const { tab, texts } = await openScreens();
    expect(texts('h2.group-title')).toEqual(['Creation', 'View', 'Transition']);
    expect(texts('button.item .item-scope')).toEqual([
      'General: every type without its own',
      'For Bug',
      'General: every type without its own',
      'Shown by Reject',
    ]);
    expect(texts('button.item .item-count')).toEqual(['3 fields', '1 fields', '2 fields', '1 fields']);
    expect(await axeViolations(tab())).toEqual([]);
  });

  it('adds, removes and reorders the fields of a screen with the keyboard alone, and previews it', async () => {
    const { tab, panel, texts, click, button, reply } = await openScreens();
    await click(tab().querySelectorAll('button.item')[2]);
    expect(texts('.on-screen .label', panel())).toEqual(['Points', 'Application']);
    expect(texts('.available .label', panel())).toEqual(['Environment', 'Rejection reason']);
    // Required for bugs in progress, missing: said so.
    expect(texts('.warning strong', panel())).toEqual(['Environment']);
    await click(button('Put Environment on the screen'));
    await click(button('Move Environment up'));
    await click(button('Take Points off the screen'));
    expect(texts('.on-screen .label', panel())).toEqual(['Environment', 'Application']);
    expect(panel().querySelector('.warning')).toBeNull();
    // The preview of a story in its first status: all visible.
    expect(texts('.lines .line', panel())).toEqual(['Environment', 'Application']);
    expect(await axeViolations(panel())).toEqual([]);

    await click(panel().querySelector('button.save'));
    const saved = await reply((url, method) => url.endsWith('/screens/view') && method === 'PUT', SCREENS[2]);
    expect(saved.request.headers.get('If-Match')).toBe('"1"');
    expect(saved.request.body).toEqual({ nameTranslations: { fr: 'Consultation', en: 'View' }, fieldIds: ['f-env', 'f-app'] });
    await reply((url, method) => url.endsWith('/screens') && method === 'GET', { stage: 'draft', items: SCREENS });
    await reply((url) => url === `/api/v1/project-configs/${CONFIG.id}`, CONFIG);
    expect(texts('[role="status"]')).toEqual(['Screen “View” saved in the draft.']);
  });

  it('shows the fields a rule hides, disabled, in the preview', async () => {
    const { tab, panel, root, texts, click, text } = await openScreens();
    await click(tab().querySelectorAll('button.item')[0]);
    // For a bug at creation: Points is not a bug's, Application hidden by its rule.
    await click(panel().querySelector('mat-select .mat-mdc-select-trigger'));
    await click(Array.from(root().querySelectorAll('mat-option')).find((o) => text(o) === 'Bug'));
    expect(texts('.lines .line .label', panel())).toEqual(['Points', 'Application', 'Environment']);
    expect(texts('.lines .line .tag', panel())).toEqual(['Hidden by a rule', 'Hidden by a rule']);
    expect(panel().querySelector('.line[data-mode="hidden"]')?.getAttribute('aria-disabled')).toBe('true');
    expect(panel().querySelector('button.remove')).toBeNull();
  });

  it('adds a creation screen for a type that has none', async () => {
    const { tab, root, click, reply, text } = await openScreens();
    await click(tab().querySelector('button.add'));
    const dialog = root().querySelector('mat-dialog-container')!;
    await click(dialog.querySelectorAll('mat-select .mat-mdc-select-trigger')[0]);
    await click(Array.from(root().querySelectorAll('mat-option')).find((o) => text(o) === 'Creation'));
    // Bug has its own creation screen already: only Story is offered.
    await click(dialog.querySelectorAll('mat-select .mat-mdc-select-trigger')[1]);
    const options = () => Array.from(Array.from(root().querySelectorAll('.mat-mdc-select-panel')).at(-1)!.querySelectorAll('mat-option'));
    expect(options().map((o) => text(o))).toEqual(['Story']);
    await click(options()[0]);
    const name = dialog.querySelector('input[lang="fr"]') as HTMLInputElement;
    name.value = 'Création d’une story';
    name.dispatchEvent(new Event('input'));
    expect(await axeViolations(dialog)).toEqual([]);
    await click(dialog.querySelector('button.confirm'));
    const created = await reply(
      (url, method) => url.endsWith('/screens') && method === 'POST',
      screen('story-creation', { issueTypeId: 't2', name: 'Création d’une story' }),
      201,
    );
    expect(created.request.body).toEqual({
      kind: 'creation',
      issueTypeId: 't2',
      nameTranslations: { fr: 'Création d’une story' },
      fieldIds: [],
    });
  });

  it('says which transitions keep a transition screen from being removed', async () => {
    const { tab, panel, root, click, reply, text } = await openScreens();
    await click(tab().querySelectorAll('button.item')[3]);
    await click(panel().querySelector('button.remove'));
    await click(Array.from(root().querySelectorAll('mat-dialog-container button')).find((b) => text(b) === 'Remove'));
    await reply(
      (url, method) => url.endsWith('/screens/reason') && method === 'DELETE',
      { type: 'urn:mlbpp:problem:screen.in_use', title: 'in use', status: 409, code: 'screen.in_use', detail: 'Reject (Standard workflow)' },
      409,
    );
    expect(text(panel().querySelector('[role="alert"]'))).toBe(
      'Transitions show this screen: give them another one first. Shown by: Reject (Standard workflow).',
    );
  });
});
