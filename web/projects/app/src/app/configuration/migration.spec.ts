/**
 * Tests of the status and type mapping wizard (L04-US08): the mapping is
 * complete only when every removed item has a target, the ticket count of
 * the confirm button, the mappings sent; then the wizard opened from the
 * publish dialog with the API mocked at the HTTP level and the real
 * translations: its table, the button disabled until every status is
 * mapped, the migration followed until the draft is published, and
 * accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { AuthService, Migration, MigrationPlan, ProjectConfig, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { ConfigPage } from './config-page';
import { initialChoices, isComplete, mappings, ticketTotal } from './migration';
import { TypesTab } from './types-tab';

const CONFIG: ProjectConfig = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a02',
  name: 'Software development',
  nameTranslations: { fr: 'Développement logiciel', en: 'Software development' },
  isDefault: false,
  projectCount: 6,
  published: { number: 2, publishedAt: '2026-11-12T09:00:00Z' },
  draft: { number: 3, changeCount: 2, createdAt: '2026-11-13T08:00:00Z' },
  version: 5,
};

const target = (id: string, label: string, isNew = false) => ({ id, label, isNew });

/** Mockup 13: "In test" goes, "In acceptance" comes; the Bug type goes too. */
const PLAN: MigrationPlan = {
  statuses: [
    {
      id: 's-test',
      key: 'in_test',
      label: 'In test',
      workflow: 'Standard workflow',
      ticketCount: 38,
      targets: [target('s-todo', 'To do'), target('s-progress', 'In progress'), target('s-acceptance', 'In acceptance', true)],
      suggestedId: 's-progress',
    },
  ],
  types: [
    {
      id: 't-bug',
      key: 'bug',
      label: 'Bug',
      workflow: null,
      ticketCount: 2134,
      targets: [target('t-task', 'Task'), target('t-story', 'Story')],
      suggestedId: null,
    },
  ],
  ticketTotal: 2172,
  version: 5,
};

const PROJECTS = [
  { id: 'p1', name: 'Identity platform', key: 'PLAT', archived: false },
  { id: 'p2', name: 'CRM', key: 'CRM', archived: false },
  { veiled: true },
];

const SESSION = {
  account: { id: 'a1', email: 'lucas@acme.example', displayName: 'Lucas Bernard', locale: null, tenantRole: 'configuration_admin' },
  stage: 'complete',
  csrfToken: 'csrf',
  expiresAt: '2026-11-20T09:00:00Z',
};

const migration = (state: Migration['state'], migrated: number, percent: number): Migration => ({
  id: 'm1',
  state,
  ticketTotal: 2172,
  ticketsMigrated: migrated,
  percent,
  error: null,
  createdAt: '2026-11-14T10:00:00Z',
  finishedAt: state === 'done' ? '2026-11-14T10:01:00Z' : null,
});

describe('the mapping of a migration', () => {
  it('is complete once every removed status and type has one of its targets', () => {
    const choices = initialChoices(PLAN);
    expect(choices).toEqual({ 's-test': 's-progress', 't-bug': null });
    expect(isComplete(PLAN, choices)).toBe(false);
    expect(isComplete(PLAN, { ...choices, 't-bug': 't-task' })).toBe(true);
    // A target not offered for it does not count.
    expect(isComplete(PLAN, { ...choices, 't-bug': 's-todo' })).toBe(false);
    expect(isComplete({ ...PLAN, statuses: [], types: [] }, {})).toBe(true);
  });

  it('counts the tickets and sends the statuses and types apart', () => {
    expect(ticketTotal(PLAN)).toBe(2172);
    expect(mappings(PLAN, { 's-test': 's-acceptance', 't-bug': 't-task' })).toEqual({
      statusMappings: [{ fromId: 's-test', toId: 's-acceptance' }],
      typeMappings: [{ fromId: 't-bug', toId: 't-task' }],
    });
  });

  it('counts a ticket in both a removed status and a removed type once, as the API does', () => {
    // 38 in "In test" and 2,134 bugs, 10 of them bugs in test: 2,162 tickets.
    expect(ticketTotal({ ...PLAN, ticketTotal: 2162 })).toBe(2162);
  });
});

describe('the migration wizard', () => {
  async function openWizard() {
    localStorage.clear();
    TestBed.configureTestingModule({
      providers: [
        provideRouter(
          [{ path: 'admin/configurations/:id', component: ConfigPage, children: [{ path: 'types', component: TypesTab }] }],
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
      for (let i = 0; i < 100 && !request; i++) {
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
    await harness.navigateByUrl(`/admin/configurations/${CONFIG.id}/types`);
    await reply((url) => url === `/api/v1/project-configs/${CONFIG.id}`, CONFIG);
    const root = () => document.body;
    const dialogs = () => Array.from(root().querySelectorAll('mat-dialog-container'));
    const wizard = () => dialogs().find((d) => d.querySelector('app-migration-dialog'))!;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const click = async (element: Element | null | undefined) => {
      (element as HTMLElement).click();
      await settle();
    };
    // Publishing: tickets use what the draft removes.
    await click(root().querySelector('button.publish'));
    await reply((url) => url.endsWith('/publish-impact'), {
      projects: PROJECTS,
      removedTypes: [],
      removedStatuses: [],
      workflowProblems: [],
      needsMigration: true,
      version: 5,
    });
    await click(root().querySelector('button.migrate'));
    await reply((url) => url.endsWith('/migration-plan'), PLAN);
    return { reply, root, wizard, dialogs, text, click };
  }

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it('lists each removed status and type with its tickets and a target, the new ones marked', async () => {
    const { wizard, root, text, click } = await openWizard();
    expect(text(wizard().querySelector('h2'))).toBe('Migrate the tickets');
    const rows = Array.from(wizard().querySelectorAll('table.mapping tbody tr'));
    expect(rows.map((r) => text(r.querySelector('th')))).toEqual(['In test Standard workflow', 'Bug']);
    expect(rows.map((r) => text(r.querySelector('td.count')))).toEqual(['38', '2,134']);
    expect(text(rows[0].querySelector('mat-select'))).toBe('In progress');
    expect(Array.from(wizard().querySelectorAll('.chip')).map((c) => text(c))).toEqual(['PLAT', 'CRM', 'lockProject you do not have access to']);
    // Bug has no target yet: nothing starts.
    const confirm = wizard().querySelector('button.confirm') as HTMLButtonElement;
    expect(text(confirm)).toBe('Migrate 2,172 tickets');
    expect(confirm.disabled).toBe(true);
    await click(rows[0].querySelector('.mat-mdc-select-trigger'));
    expect(Array.from(root().querySelectorAll('mat-option')).map((o) => text(o))).toEqual([
      'To do',
      'In progress',
      'In acceptance (new)',
    ]);
    expect(await axeViolations(wizard())).toEqual([]);
  });

  it('migrates once every status and type is mapped, and closes once the draft is published', async () => {
    const { wizard, dialogs, root, reply, text, click } = await openWizard();
    const rows = () => Array.from(wizard().querySelectorAll('table.mapping tbody tr'));
    const pick = async (row: number, label: string) => {
      await click(rows()[row].querySelector('.mat-mdc-select-trigger'));
      await click(Array.from(root().querySelectorAll('mat-option')).find((o) => text(o) === label));
    };
    await pick(0, 'In acceptance (new)');
    await pick(1, 'Task');
    const confirm = wizard().querySelector('button.confirm') as HTMLButtonElement;
    expect(confirm.disabled).toBe(false);
    await click(confirm);
    const started = await reply((url, method) => url.endsWith('/migrations') && method === 'POST', migration('pending', 0, 0), 202);
    expect(started.request.headers.get('If-Match')).toBe('"5"');
    expect(started.request.body).toEqual({
      statusMappings: [{ fromId: 's-test', toId: 's-acceptance' }],
      typeMappings: [{ fromId: 't-bug', toId: 't-task' }],
    });
    // Followed: halfway, then done.
    await reply((url) => url.endsWith('/migrations/m1'), migration('running', 1086, 50));
    expect(text(wizard().querySelector('.progress p'))).toBe('1,086 of 2,172 tickets migrated…');
    expect(wizard().querySelector('mat-progress-bar')?.getAttribute('aria-valuenow')).toBe('50');
    expect(wizard().querySelector('button.confirm')).toBeNull();
    expect(await axeViolations(wizard())).toEqual([]);
    await reply((url) => url.endsWith('/migrations/m1'), migration('done', 2172, 100));
    await reply((url) => url === `/api/v1/project-configs/${CONFIG.id}`, { ...CONFIG, draft: null, published: { number: 3, publishedAt: '2026-11-14T10:01:00Z' } });
    // Both dialogs closed: the configuration is published.
    for (let i = 0; i < 20 && dialogs().length > 0; i++) await new Promise((resolve) => setTimeout(resolve, 20));
    expect(dialogs()).toHaveLength(0);
  });

  it('says when the mapping was refused or the migration failed', async () => {
    const { wizard, reply, text, click, root } = await openWizard();
    await click(Array.from(wizard().querySelectorAll('table.mapping tbody tr'))[1].querySelector('.mat-mdc-select-trigger'));
    await click(Array.from(root().querySelectorAll('mat-option')).find((o) => text(o) === 'Story'));
    await click(wizard().querySelector('button.confirm'));
    await reply(
      (url, method) => url.endsWith('/migrations') && method === 'POST',
      { type: 'urn:mlbpp:problem:config.migration_running', title: 'running', status: 409, code: 'config.migration_running' },
      409,
    );
    expect(text(wizard().querySelector('[role="alert"]'))).toBe('A migration of this configuration is already under way.');
    await click(wizard().querySelector('button.confirm'));
    await reply((url, method) => url.endsWith('/migrations') && method === 'POST', migration('pending', 0, 0), 202);
    await reply((url) => url.endsWith('/migrations/m1'), { ...migration('failed', 0, 0), error: 'the draft changed since the mapping' });
    expect(text(wizard().querySelector('.progress'))).toBe(
      'The migration stopped, nothing was published: the draft changed since the mapping.',
    );
  });
});
