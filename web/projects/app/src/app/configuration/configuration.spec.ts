/**
 * Integration tests of the project configuration screens (L04-US01): the
 * Administration tabs by tenant role, the configurations list and the
 * creation dialog, a configuration's page with its chips, its sections and
 * its Projects tab, the publication after its impact, the draft discarded,
 * duplication and detachment, and the Configuration tab of a project's
 * settings, with the API mocked at the HTTP level and the real
 * translations.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { Router, provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import {
  AuthService,
  ConfigProjectItem,
  Node,
  NodeConfiguration,
  ProjectConfig,
  ProjectConfigPublishImpact,
  provideApi,
  provideMaterialDefaults,
} from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { AdminPage } from '../admin/admin-page';
import { adminHomeGuard } from '../admin/admin-tabs';
import { NodeConfigurationTab } from '../hierarchy/node-configuration-tab';
import { NodeSettingsPage } from '../hierarchy/node-settings-page';
import { copyName } from './config-dialog';
import { ConfigPage } from './config-page';
import { ConfigProjectsTab } from './config-projects-tab';
import { ConfigSectionTab } from './config-section-tab';
import { ConfigurationsTab } from './configurations-tab';
import { TypesTab } from './types-tab';

const DEFAULT: ProjectConfig = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a01',
  name: 'Default configuration',
  nameTranslations: { fr: 'Configuration par défaut', en: 'Default configuration' },
  isDefault: true,
  projectCount: 3,
  published: { number: 1, publishedAt: '2026-11-10T09:00:00Z' },
  draft: null,
  version: 1,
};

const SOFTWARE: ProjectConfig = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a02',
  name: 'Software development',
  nameTranslations: { fr: 'Développement logiciel', en: 'Software development' },
  isDefault: false,
  projectCount: 6,
  published: { number: 2, publishedAt: '2026-11-12T09:00:00Z' },
  draft: { number: 3, changeCount: 2, createdAt: '2026-11-13T08:00:00Z' },
  version: 5,
};

const PLAT = { id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7c03', name: 'Identity platform', key: 'PLAT', archived: false };
const LEGACY = { id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7c04', name: 'Legacy CRM', key: 'CRM', archived: true };
const VEILED = { veiled: true, id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7c99' };
const PROJECTS: ConfigProjectItem[] = [PLAT, LEGACY, VEILED];

const PROJECT_NODE: Node = {
  id: PLAT.id,
  type: 'project',
  name: 'Identity platform',
  key: 'PLAT',
  description: '',
  objectives: '',
  startDate: null,
  endDate: null,
  health: 'on_track',
  archived: false,
  confidential: false,
  owners: [],
  parents: [],
  version: 1,
};

const USED: NodeConfiguration = { id: SOFTWARE.id, name: 'Software development', projectCount: 6, isDefault: false };

const SESSION = (tenantRole: string) => ({
  account: { id: 'a1', email: 'lucas@acme.example', displayName: 'Lucas Bernard', locale: null, tenantRole },
  stage: 'complete',
  csrfToken: 'csrf',
  expiresAt: '2026-11-20T09:00:00Z',
});

function problem(status: number, code: string, errors: object[] = []) {
  return { type: `urn:mlbpp:problem:${code}`, title: code, status, code, errors };
}

const page = (items: ProjectConfig[]) => ({ items, nextCursor: null });
const isList = (url: string, method: string) => url.startsWith('/api/v1/project-configs?') && method === 'GET';
const isConfig = (id: string) => (url: string, method: string) => url === `/api/v1/project-configs/${id}` && method === 'GET';
const isProjects = (id: string) => (url: string, method: string) =>
  url === `/api/v1/project-configs/${id}/projects` && method === 'GET';
const isImpact = (id: string) => (url: string) => url === `/api/v1/project-configs/${id}/publish-impact`;

async function setUp(tenantRole = 'tenant_admin') {
  localStorage.clear();
  TestBed.configureTestingModule({
    providers: [
      provideRouter(
        [
          {
            path: 'admin/configurations/:id',
            component: ConfigPage,
            children: [
              { path: '', pathMatch: 'full', redirectTo: 'types' },
              { path: 'types', component: TypesTab },
              { path: 'notifications', component: ConfigSectionTab, data: { section: 'notifications', icon: 'notifications' } },
              { path: 'projects', component: ConfigProjectsTab },
            ],
          },
          {
            path: 'admin',
            component: AdminPage,
            children: [
              { path: '', pathMatch: 'full', canActivate: [adminHomeGuard], children: [] },
              { path: 'configurations', component: ConfigurationsTab },
            ],
          },
          {
            path: 'nodes/:id',
            component: NodeSettingsPage,
            children: [{ path: 'configuration', component: NodeConfigurationTab }],
          },
        ],
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
  const auth = TestBed.inject(AuthService);
  const loaded = auth.load();
  await reply((url) => url === '/api/v1/auth/session', SESSION(tenantRole));
  await loaded;
  const harness = await RouterTestingHarness.create();
  const root = () => document.body;
  const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
  const texts = (selector: string, within: Element = root()) => Array.from(within.querySelectorAll(selector)).map((e) => text(e));
  const click = async (element: Element | null) => {
    (element as HTMLElement).click();
    await settle();
  };
  const dialog = () => Array.from(root().querySelectorAll('mat-dialog-container')).at(-1)!;
  const button = (within: Element, label: string) =>
    Array.from(within.querySelectorAll('button')).find((b) => text(b)?.endsWith(label))!;
  const fill = async (within: Element, lang: string, value: string) => {
    const input = within.querySelector(`input[lang="${lang}"]`) as HTMLInputElement;
    input.value = value;
    input.dispatchEvent(new Event('input'));
    input.dispatchEvent(new Event('blur'));
    await settle();
  };
  const url = () => TestBed.inject(Router).url;
  /** Waits until `done` holds: a dialog closes after its animation. */
  const waitFor = async (done: () => boolean) => {
    for (let i = 0; i < 50 && !done(); i++) {
      await new Promise((resolve) => setTimeout(resolve, 20));
      await settle();
    }
  };
  return { http, harness, reply, settle, root, text, texts, click, dialog, button, fill, url, waitFor };
}

afterEach(() => {
  TestBed.inject(MatDialog).closeAll();
});

describe('Administration tabs', () => {
  it('lead configuration admins to the project configurations, their only tab', async () => {
    const { harness, reply, texts, url } = await setUp('configuration_admin');
    await harness.navigateByUrl('/admin');
    await reply((u) => u === '/api/v1/tenant', { id: 't1', slug: 'acme', name: 'Acme' });
    await reply(isList, page([DEFAULT]));
    expect(url()).toBe('/admin/configurations');
    expect(texts('a[mat-tab-link]')).toEqual(['Project configurations']);
  });

  it('show tenant admins every tab, the configurations after the authentication', async () => {
    const { harness, reply, texts } = await setUp('tenant_admin');
    await harness.navigateByUrl('/admin/configurations');
    await reply((u) => u === '/api/v1/tenant', { id: 't1', slug: 'acme', name: 'Acme' });
    await reply(isList, page([DEFAULT]));
    expect(texts('a[mat-tab-link]')).toEqual(['Members', 'Groups', 'Roles', 'Authentication', 'Project configurations']);
  });
});

describe('ConfigurationsTab', () => {
  async function openList(configs: ProjectConfig[]) {
    const context = await setUp();
    await context.harness.navigateByUrl('/admin/configurations');
    await context.reply((u) => u === '/api/v1/tenant', { id: 't1', slug: 'acme', name: 'Acme' });
    await context.reply(isList, page(configs));
    return context;
  }

  it('lists the configurations with their version, sharing and draft', async () => {
    const { root, text, texts } = await openList([DEFAULT, SOFTWARE]);
    const rows = Array.from(root().querySelectorAll('tr[mat-row]'));
    expect(rows.map((row) => text(row.querySelector('a.name')))).toEqual(['Default configuration', 'Software development']);
    expect(rows[0].querySelector('a.name')?.getAttribute('href')).toBe(`/admin/configurations/${DEFAULT.id}`);
    expect(texts('.secondary')).toEqual(['Version 1', 'Version 2']);
    expect(texts('.chip', rows[0])).toEqual(['starDefault', 'shareShared by 3 projects']);
    expect(texts('.chip', rows[1])).toEqual(['shareShared by 6 projects', 'edit_noteUnpublished draft · 2 changes']);
    // Neither can go: one is the default, the other is used.
    expect(root().querySelector('button.delete')).toBeNull();
    expect(await axeViolations(root().querySelector('app-configurations-tab')!)).toEqual([]);
  });

  it('creates a configuration named in the fallback language at least, then opens it', async () => {
    const { root, reply, click, dialog, fill, text, texts, url, waitFor } = await openList([DEFAULT]);
    await click(root().querySelector('button.create'));
    expect(text(dialog().querySelector('h2'))).toBe('New project configuration');
    expect(texts('mat-label', dialog())).toEqual(['Name (Français)', 'Name (English)']);

    // Nothing in French: the fallback language is required.
    await fill(dialog(), 'en', 'Software development');
    await click(dialog().querySelector('button.confirm'));
    expect(text(dialog().querySelector('mat-error'))).toBe('Required in Français, the fallback language.');
    expect(await axeViolations(dialog())).toEqual([]);

    // Taken: said on both languages.
    await fill(dialog(), 'fr', 'Développement logiciel');
    await click(dialog().querySelector('button.confirm'));
    const refused = await reply(
      (u, m) => u === '/api/v1/project-configs' && m === 'POST',
      problem(409, 'config.name_taken', [{ field: 'nameTranslations', code: 'taken' }]),
      409,
    );
    expect(refused.request.body).toEqual({ nameTranslations: { fr: 'Développement logiciel', en: 'Software development' } });
    const key = refused.request.headers.get('Idempotency-Key');
    expect(key).toBeTruthy();
    expect(texts('mat-error', dialog())).toEqual(['This value is already used.', 'This value is already used.']);

    await fill(dialog(), 'en', 'Software');
    await click(dialog().querySelector('button.confirm'));
    const created = await reply((u, m) => u === '/api/v1/project-configs' && m === 'POST', { ...SOFTWARE, draft: null }, 201);
    expect(created.request.headers.get('Idempotency-Key')).toBe(key);
    await waitFor(() => url().startsWith(`/admin/configurations/${SOFTWARE.id}`));
    expect(url()).toBe(`/admin/configurations/${SOFTWARE.id}/types`);
  });

  it('deletes an unused configuration after a confirmation', async () => {
    const unused = { ...SOFTWARE, projectCount: 0, draft: null };
    const { root, reply, click, dialog, button, text } = await openList([DEFAULT, unused]);
    const remove = root().querySelector('button.delete')!;
    expect(remove.getAttribute('aria-label')).toBe('Delete Software development');
    await click(remove);
    expect(text(dialog().querySelector('h2'))).toBe('Delete “Software development”?');
    await click(button(dialog(), 'Delete'));
    const deleted = await reply((u, m) => u === `/api/v1/project-configs/${unused.id}` && m === 'DELETE', null, 204);
    expect(deleted.request.headers.get('If-Match')).toBe('"5"');
    expect(text(root().querySelector('[role="status"]'))).toBe('Configuration “Software development” deleted.');
  });
});

describe('ConfigPage', () => {
  async function openConfig(config: ProjectConfig, tab = '') {
    const context = await setUp();
    await context.harness.navigateByUrl(`/admin/configurations/${config.id}${tab ? `/${tab}` : ''}`);
    await context.reply(isConfig(config.id), config);
    return context;
  }

  it('shows the sharing and the draft, the sections and the projects in tabs', async () => {
    const { root, text, texts, url, harness } = await openConfig(SOFTWARE);
    // It opens on its types.
    expect(url()).toBe(`/admin/configurations/${SOFTWARE.id}/types`);
    expect(root().querySelector('app-types-tab')).not.toBeNull();
    await harness.navigateByUrl(`/admin/configurations/${SOFTWARE.id}/notifications`);
    expect(text(root().querySelector('h1'))).toBe('Software development');
    expect(texts('nav.eyebrow a')).toEqual(['Administration', 'Project configurations']);
    expect(texts('app-config-page .chip')).toEqual(['shareShared by 6 projects', 'edit_noteUnpublished draft · 2 changes']);
    expect(texts('a[mat-tab-link]')).toEqual(['Types', 'Fields', 'Workflows', 'Screens', 'Notifications', 'Projects (6)']);
    // A section of a later story.
    expect(text(root().querySelector('app-config-section-tab'))).toContain('Notifications come next');
    expect((root().querySelector('button.publish') as HTMLButtonElement).disabled).toBe(false);
    expect(await axeViolations(root().querySelector('app-config-page')!)).toEqual([]);
  });

  it('offers nothing to publish without a draft', async () => {
    const { root, text } = await openConfig({ ...SOFTWARE, draft: null });
    const publish = root().querySelector('button.publish') as HTMLButtonElement;
    expect(publish.disabled).toBe(true);
    expect(publish.getAttribute('aria-describedby')).toBe('publish-hint');
    expect(text(root().querySelector('#publish-hint'))).toBe(
      'Version 2 is published. Changes go to a draft until they are published.',
    );
    expect(root().querySelector('button.discard')).toBeNull();
  });

  it('publishes once the affected projects are confirmed, with the version they were read at', async () => {
    const { root, reply, click, dialog, text, texts, waitFor } = await openConfig(SOFTWARE);
    await click(root().querySelector('button.publish'));
    expect(text(dialog().querySelector('h2'))).toBe('Publish “Software development”?');
    expect(dialog().querySelector('mat-progress-bar')).not.toBeNull();
    expect((dialog().querySelector('button.confirm') as HTMLButtonElement).disabled).toBe(true);
    const impact: ProjectConfigPublishImpact = {
      projects: PROJECTS,
      removedTypes: [{ id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b05', key: 'bug', label: 'Bug', ticketCount: 0 }], removedStatuses: [], workflowProblems: [],
      needsMigration: false,
      version: 6,
    };
    await reply(isImpact(SOFTWARE.id), impact);
    expect(text(dialog().querySelector('.intro'))).toBe('The new version applies at once to these 3 projects:');
    const items = Array.from(dialog().querySelectorAll('.projects li'));
    expect(items.map((li) => [text(li.querySelector('.name')), text(li.querySelector('.key'))])).toEqual([
      ['Identity platform', 'PLAT'],
      ['Legacy CRM', 'CRM'],
      [undefined, undefined],
    ]);
    expect(text(items[1].querySelector('.archived'))).toBe('Archived');
    expect(text(items[2].querySelector('.veiled'))).toBe('Project you do not have access to');
    // The types the draft removes, with the tickets still using them.
    expect(text(dialog().querySelector('h3.sub'))).toBe('Removed types');
    const removed = dialog().querySelector('.removed li')!;
    expect([text(removed.querySelector('.name')), text(removed.querySelector('.key')), text(removed.querySelector('.archived'))]).toEqual([
      'Bug',
      'bug',
      '0 tickets',
    ]);
    expect(await axeViolations(dialog())).toEqual([]);

    await click(dialog().querySelector('button.confirm'));
    const published = await reply(
      (u, m) => u === `/api/v1/project-configs/${SOFTWARE.id}/publish` && m === 'POST',
      { ...SOFTWARE, published: { number: 3, publishedAt: '2026-11-13T10:00:00Z' }, draft: null, version: 7 },
    );
    expect(published.request.headers.get('If-Match')).toBe('"6"');
    await waitFor(() => root().querySelector('mat-dialog-container') === null);
    expect(root().querySelector('mat-dialog-container')).toBeNull();
    expect(text(root().querySelector('[role="status"]'))).toBe('Version 3 is published to its 6 projects.');
    expect(texts('app-config-page .chip')).toEqual(['shareShared by 6 projects']);
  });

  it('reads the projects again when the configuration changed in the meantime', async () => {
    const { root, reply, click, dialog, text } = await openConfig(SOFTWARE);
    await click(root().querySelector('button.publish'));
    await reply(isImpact(SOFTWARE.id), { projects: [PLAT], removedTypes: [], removedStatuses: [], workflowProblems: [], needsMigration: false, version: 6 });
    expect(text(dialog().querySelector('.intro'))).toBe('The new version applies at once to this project:');
    await click(dialog().querySelector('button.confirm'));
    await reply((u, m) => u.endsWith('/publish') && m === 'POST', problem(412, 'precondition_failed'), 412);
    expect(text(dialog().querySelector('[role="alert"]'))).toBe(
      'The configuration changed in the meantime: check the projects again, then publish.',
    );
    await reply(isImpact(SOFTWARE.id), { projects: [PLAT, LEGACY], removedTypes: [], removedStatuses: [], workflowProblems: [], needsMigration: false, version: 8 });
    await click(dialog().querySelector('button.confirm'));
    const again = await reply((u, m) => u.endsWith('/publish') && m === 'POST', { ...SOFTWARE, draft: null });
    expect(again.request.headers.get('If-Match')).toBe('"8"');
  });

  it('does not publish while tickets must be migrated, and says when no project is affected', async () => {
    const { root, reply, click, dialog, text } = await openConfig(SOFTWARE);
    await click(root().querySelector('button.publish'));
    await reply(isImpact(SOFTWARE.id), { projects: [PLAT], removedTypes: [], removedStatuses: [], workflowProblems: [], needsMigration: true, version: 6 });
    expect(text(dialog().querySelector('[role="alert"]'))).toBe(
      'Some tickets use a type or a status this version removes: they must be migrated first.',
    );
    expect((dialog().querySelector('button.confirm') as HTMLButtonElement).disabled).toBe(true);
    TestBed.inject(MatDialog).closeAll();

    await click(root().querySelector('button.publish'));
    await reply(isImpact(SOFTWARE.id), { projects: [], removedTypes: [], removedStatuses: [], workflowProblems: [], needsMigration: false, version: 6 });
    expect(text(dialog().querySelector('.intro'))).toBe(
      'No project uses this configuration yet: the new version will apply to the projects moved to it.',
    );
    expect((dialog().querySelector('button.confirm') as HTMLButtonElement).disabled).toBe(false);
  });

  it('discards the draft after a confirmation', async () => {
    const { root, reply, click, dialog, button, text, texts } = await openConfig(SOFTWARE);
    await click(root().querySelector('button.discard'));
    expect(text(dialog().querySelector('h2'))).toBe('Discard the draft of “Software development”?');
    await click(button(dialog(), 'Discard'));
    const discarded = await reply(
      (u, m) => u === `/api/v1/project-configs/${SOFTWARE.id}/draft` && m === 'DELETE',
      { ...SOFTWARE, draft: null, version: 6 },
    );
    expect(discarded.request.headers.get('If-Match')).toBe('"5"');
    expect(text(root().querySelector('[role="status"]'))).toBe('The draft is discarded.');
    expect(texts('app-config-page .chip')).toEqual(['shareShared by 6 projects']);
  });

  it('duplicates the configuration under a new name, then opens the copy', async () => {
    const { root, reply, click, dialog, text, url, waitFor } = await openConfig(SOFTWARE);
    await click(root().querySelector('button.duplicate'));
    expect(text(dialog().querySelector('h2'))).toBe('Duplicate “Software development”');
    const values = Array.from(dialog().querySelectorAll('input')).map((input) => input.value);
    expect(values).toEqual(['Développement logiciel (2)', 'Software development (2)']);
    await click(dialog().querySelector('button.confirm'));
    const copy = { ...SOFTWARE, id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a03', projectCount: 0, draft: null };
    const duplicated = await reply((u, m) => u === `/api/v1/project-configs/${SOFTWARE.id}/duplicate` && m === 'POST', copy, 201);
    expect(duplicated.request.body).toEqual({
      nameTranslations: { fr: 'Développement logiciel (2)', en: 'Software development (2)' },
      projectId: null,
    });
    await waitFor(() => url().startsWith(`/admin/configurations/${copy.id}`));
    expect(url()).toBe(`/admin/configurations/${copy.id}/types`);
  });
});

describe('ConfigProjectsTab', () => {
  async function openProjects(config: ProjectConfig, projects: ConfigProjectItem[]) {
    const context = await setUp();
    await context.harness.navigateByUrl(`/admin/configurations/${config.id}/projects`);
    await context.reply(isConfig(config.id), config);
    await context.reply(isProjects(config.id), { items: projects });
    return context;
  }

  it('lists the projects sharing it, veiled when out of reach, archived ones marked', async () => {
    const { root, text } = await openProjects(SOFTWARE, PROJECTS);
    const rows = Array.from(root().querySelectorAll('tr[mat-row]'));
    expect(rows.map((row) => [text(row.querySelector('.name')), text(row.querySelector('.secondary'))])).toEqual([
      ['Identity platform', 'PLAT'],
      ['Legacy CRM', 'CRM'],
      ['Confidential project', undefined],
    ]);
    expect(rows[2].querySelector('app-veiled-name mat-icon')?.getAttribute('aria-label')).toBe('Locked');
    expect(rows[0].querySelector('a')?.getAttribute('href')).toBe(`/nodes/${PLAT.id}`);
    expect(text(rows[1].querySelector('.archived'))).toBe('Archived');
    expect(rows[2].querySelector('a')).toBeNull();
    // Only an active project it sees detaches.
    expect(rows.map((row) => row.querySelector('button.detach') !== null)).toEqual([true, false, false]);
    expect(rows[0].querySelector('button.detach')?.getAttribute('aria-label')).toBe('Detach Identity platform');
    expect(await axeViolations(root().querySelector('app-config-projects-tab')!)).toEqual([]);
  });

  it('detaches a project into its own copy of the configuration', async () => {
    const { root, reply, click, dialog, text, waitFor } = await openProjects(SOFTWARE, PROJECTS);
    await click(root().querySelector('button.detach'));
    expect(text(dialog().querySelector('h2'))).toBe('Detach Identity platform');
    expect(text(dialog().querySelector('.intro'))).toBe(
      'Identity platform gets its own copy of “Software development”: further changes to the original no longer affect it.',
    );
    await click(dialog().querySelector('button.confirm'));
    const copy = { ...SOFTWARE, id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a04', name: 'Software development (PLAT)', projectCount: 1 };
    const detached = await reply((u, m) => u.endsWith('/duplicate') && m === 'POST', copy, 201);
    expect(detached.request.body).toEqual({
      nameTranslations: { fr: 'Développement logiciel (PLAT)', en: 'Software development (PLAT)' },
      projectId: PLAT.id,
    });
    await waitFor(() => root().querySelector('[role="status"]') !== null);
    expect(text(root().querySelector('[role="status"]'))).toBe(
      'Identity platform now uses its own configuration “Software development (PLAT)”.',
    );
    await reply(isProjects(SOFTWARE.id), { items: [LEGACY, VEILED] });
    await reply(isConfig(SOFTWARE.id), { ...SOFTWARE, projectCount: 5 });
    expect(text(root().querySelector('a[mat-tab-link]:last-child'))).toBe('Projects (5)');
  });

  it('says when no project uses it, and offers no detachment when it is not shared', async () => {
    const { root, text } = await openProjects({ ...SOFTWARE, projectCount: 0 }, []);
    expect(text(root().querySelector('ui-empty-state'))).toContain('No project uses this configuration');
    TestBed.resetTestingModule();
    const single = await openProjects({ ...SOFTWARE, projectCount: 1 }, [PLAT]);
    expect(single.root().querySelector('button.detach')).toBeNull();
  });

  it('names a copy after the original, by language', () => {
    expect(copyName({ mode: 'create' })).toEqual({});
    expect(copyName({ mode: 'duplicate', source: { ...SOFTWARE, nameTranslations: { fr: ' Support ', en: '' } } })).toEqual({
      fr: 'Support (2)',
    });
    expect(copyName({ mode: 'detach', source: SOFTWARE, project: PLAT })).toEqual({
      fr: 'Développement logiciel (PLAT)',
      en: 'Software development (PLAT)',
    });
  });
});

describe('NodeConfigurationTab', () => {
  async function openTab(tenantRole: string, node: Node = PROJECT_NODE) {
    const context = await setUp(tenantRole);
    await context.harness.navigateByUrl(`/nodes/${node.id}/configuration`);
    await context.reply((u) => u === `/api/v1/nodes/${node.id}`, node);
    await context.reply((u) => u === `/api/v1/nodes/${node.id}/configuration`, USED);
    return context;
  }

  it('shows the configuration the project uses, and lets admins move it to another one', async () => {
    const { root, reply, click, text, texts, settle } = await openTab('configuration_admin');
    await reply(isList, page([DEFAULT, SOFTWARE]));
    const tab = root().querySelector('app-node-configuration-tab')!;
    expect(texts('a[mat-tab-link]')).toEqual(['General', 'Attachments and permissions', 'Configuration', 'Archiving']);
    expect(text(tab.querySelector('.uses'))).toBe('This project uses Software development');
    expect(tab.querySelector('a.config-name')?.getAttribute('href')).toBe(`/admin/configurations/${SOFTWARE.id}`);
    expect(texts('.chip', tab)).toEqual(['shareShared by 6 projects']);
    const save = tab.querySelector('button.save') as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    expect(await axeViolations(tab)).toEqual([]);

    await click(tab.querySelector('mat-select .mat-mdc-select-trigger'));
    await click(Array.from(root().querySelectorAll('mat-option')).find((o) => text(o) === 'Default configuration')!);
    await settle();
    expect(save.disabled).toBe(false);
    await click(save);
    const moved = await reply(
      (u, m) => u === `/api/v1/nodes/${PLAT.id}/configuration` && m === 'PUT',
      { id: DEFAULT.id, name: 'Default configuration', projectCount: 4, isDefault: true },
    );
    expect(moved.request.body).toEqual({ configId: DEFAULT.id });
    expect(text(tab.querySelector('[role="status"]'))).toBe('The project now uses “Default configuration”.');
    expect(text(tab.querySelector('.uses'))).toBe('This project uses Default configuration');
    expect(texts('.chip', tab)).toEqual(['starDefault', 'shareShared by 4 projects']);
  });

  it('is read-only for the other members', async () => {
    const { root, text } = await openTab('member');
    const tab = root().querySelector('app-node-configuration-tab')!;
    expect(text(tab.querySelector('.uses'))).toBe('This project uses Software development');
    expect(tab.querySelector('a.config-name')).toBeNull();
    expect(tab.querySelector('mat-select')).toBeNull();
    expect(text(tab.querySelector('.hint:last-child'))).toBe('Tenant admins and configuration admins change it.');
    expect(await axeViolations(tab)).toEqual([]);
  });

  it('keeps the configuration of an archived project', async () => {
    const { root, reply, text } = await openTab('tenant_admin', { ...PROJECT_NODE, archived: true });
    await reply(isList, page([DEFAULT, SOFTWARE]));
    const tab = root().querySelector('app-node-configuration-tab')!;
    expect(tab.querySelector('mat-select')?.getAttribute('aria-disabled')).toBe('true');
    expect(text(tab.querySelector('mat-hint'))).toBe('An archived project keeps its configuration until it is restored.');
    expect((tab.querySelector('button.save') as HTMLButtonElement).disabled).toBe(true);
  });
});
