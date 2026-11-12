/**
 * Integration tests of the hierarchy screens (L03-US01): the portfolios
 * list, the creation dialog, a node's settings and its General tab, with
 * the API mocked at the HTTP level and the real translations.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { Router, provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { AuthService, Node, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { NodeGeneralTab } from './node-general-tab';
import { NodeSettingsPage } from './node-settings-page';
import { PortfoliosPage } from './portfolios-page';

const PROGRAM: Node = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7c02',
  type: 'program',
  name: 'IT Modernization',
  key: null,
  description: '',
  objectives: '',
  startDate: '2026-01-05',
  endDate: '2026-12-18',
  health: 'at_risk',
  archived: false,
  confidential: false,
  owners: [{ accountId: 'a1', displayName: 'Lucas Bernard' }],
  parents: [],
  version: 3,
};

const PROJECT: Node = {
  ...PROGRAM,
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7c03',
  type: 'project',
  name: 'Identity platform',
  key: 'PLAT',
  health: 'on_track',
  startDate: null,
  endDate: null,
  owners: [],
  parents: [{ id: PROGRAM.id, type: 'program', name: 'IT Modernization' }],
  version: 1,
};

const SESSION = (tenantRole: string) => ({
  account: { id: 'a1', email: 'lucas@acme.example', displayName: 'Lucas Bernard', locale: null, tenantRole },
  stage: 'complete',
  csrfToken: 'csrf',
  expiresAt: '2026-11-20T09:00:00Z',
});

function problem(status: number, code: string, errors: object[] = []) {
  return { type: `urn:mlbpp:problem:${code}`, title: code, status, code, errors };
}

async function setUp(tenantRole = 'tenant_admin') {
  localStorage.clear();
  TestBed.configureTestingModule({
    providers: [
      provideRouter(
        [
          { path: 'portfolios', component: PortfoliosPage },
          {
            path: 'nodes/:id',
            component: NodeSettingsPage,
            children: [
              { path: '', pathMatch: 'full', redirectTo: 'general' },
              { path: 'general', component: NodeGeneralTab },
            ],
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
  // The signed-in member.
  const auth = TestBed.inject(AuthService);
  const loaded = auth.load();
  await reply((url) => url === '/api/v1/auth/session', SESSION(tenantRole));
  await loaded;
  const harness = await RouterTestingHarness.create();
  const root = () => document.body;
  const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
  const click = async (element: Element | null) => {
    (element as HTMLElement).click();
    await settle();
  };
  const dialog = () => Array.from(root().querySelectorAll('mat-dialog-container')).at(-1)!;
  const type = async (container: Element, name: string, value: string) => {
    const input = container.querySelector(`[formcontrolname="${name}"]`) as HTMLInputElement;
    input.value = value;
    input.dispatchEvent(new Event('input'));
    input.dispatchEvent(new Event('blur'));
    await settle();
  };
  const choose = async (container: Element, name: string, label: string) => {
    await click(container.querySelector(`mat-select[formcontrolname="${name}"] .mat-mdc-select-trigger`));
    await click(Array.from(root().querySelectorAll('mat-option')).find((o) => text(o) === label)!);
  };
  return { http, harness, reply, settle, root, text, click, dialog, type, choose };
}

const isList = (url: string, method: string) => url.startsWith('/api/v1/nodes?') && method === 'GET';

describe('PortfoliosPage', () => {
  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify();
  });

  it('lists the nodes with their kind, health, owners and dates, filtered by type', async () => {
    const { harness, reply, root, text, click } = await setUp();
    await harness.navigateByUrl('/portfolios');
    await reply(isList, { items: [PROGRAM, PROJECT], nextCursor: null });
    const rows = Array.from(root().querySelectorAll('tr[mat-row]'));
    expect(rows.map((row) => text(row.querySelector('.name')))).toEqual(['IT Modernization', 'Identity platform']);
    expect(text(rows[0].querySelector('ui-node-type-chip'))).toBe('Type: Program');
    expect(text(rows[0].querySelector('ui-health-chip'))).toBe('Health: At risk');
    expect(text(rows[0].querySelectorAll('td')[1])).toBe('Root');
    expect(text(rows[0].querySelectorAll('td')[3])).toBe('Lucas Bernard');
    expect(text(rows[0].querySelectorAll('td')[4])).toBe('Jan 5, 2026 – Dec 18, 2026');
    expect(text(rows[1].querySelector('.node-key'))).toBe('PLAT');
    expect(text(rows[1].querySelectorAll('td')[1])).toBe('IT Modernization');
    expect(rows[1].querySelector('a')?.getAttribute('href')).toBe(`/nodes/${PROJECT.id}`);
    expect(await axeViolations(root().querySelector('app-portfolios-page')!)).toEqual([]);

    const project = Array.from(root().querySelectorAll('mat-chip-option')).find((c) => text(c) === 'Project')!;
    await click(project.querySelector('.mdc-evolution-chip__action'));
    expect(root().querySelectorAll('tr[mat-row]').length).toBe(1);
  });

  it('only offers creation at the root to tenant admins', async () => {
    const { harness, reply, root } = await setUp('member');
    await harness.navigateByUrl('/portfolios');
    await reply(isList, { items: [], nextCursor: null });
    expect(root().querySelector('button.create')).toBeNull();
    expect(root().querySelector('ui-empty-state')).not.toBeNull();
  });

  it('creates a project at the root, its key in capitals, then opens it', async () => {
    const { harness, reply, root, text, click, dialog, type, choose } = await setUp();
    await harness.navigateByUrl('/portfolios');
    await reply(isList, { items: [], nextCursor: null });
    await click(root().querySelector('button.create'));
    expect(text(dialog().querySelector('h2'))).toBe('New node');
    expect(dialog().querySelector('[formcontrolname="key"]')).toBeNull();
    await choose(dialog(), 'type', 'Project');
    await type(dialog(), 'name', ' Identity platform ');
    await type(dialog(), 'key', ' plat');
    expect((dialog().querySelector('[formcontrolname="key"]') as HTMLInputElement).value).toBe('PLAT');
    expect(await axeViolations(dialog())).toEqual([]);

    await click(dialog().querySelector('button[type="submit"]'));
    const taken = await reply(
      (url) => url === '/api/v1/nodes',
      problem(409, 'node.key_taken', [{ field: 'key', code: 'taken' }]),
      409,
    );
    expect(taken.request.body).toEqual({
      type: 'project',
      name: 'Identity platform',
      key: 'PLAT',
      startDate: null,
      endDate: null,
      ownerIds: [],
      parentId: null,
    });
    expect(taken.request.headers.get('Idempotency-Key')).toMatch(/^[0-9a-f-]{36}$/);
    expect(text(dialog().querySelector('mat-error'))).toBe('This value is already used.');

    await type(dialog(), 'key', 'PLAT2');
    await click(dialog().querySelector('button[type="submit"]'));
    await reply((url) => url === '/api/v1/nodes', { ...PROJECT, key: 'PLAT2', parents: [] }, 201);
    for (let i = 0; i < 50 && TestBed.inject(Router).url === '/portfolios'; i++) {
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    expect(TestBed.inject(Router).url).toBe(`/nodes/${PROJECT.id}/general`);
    await reply((url) => url === `/api/v1/nodes/${PROJECT.id}`, PROJECT);
  });
});

describe('NodeSettingsPage and NodeGeneralTab', () => {
  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify();
  });

  async function openSettings(node: Node = PROGRAM) {
    const context = await setUp();
    await context.harness.navigateByUrl(`/nodes/${node.id}`);
    await context.reply((url) => url === `/api/v1/nodes/${node.id}`, node);
    return context;
  }

  it('shows the node in its breadcrumb and title, with the General tab filled', async () => {
    const { root, text } = await openSettings(PROJECT);
    expect(text(root().querySelector('h1'))).toBe('Identity platform · Settings');
    const crumbs = Array.from(root().querySelectorAll('nav.eyebrow a')).map((a) => [text(a), a.getAttribute('href')]);
    expect(crumbs).toEqual([
      ['Portfolios', '/portfolios'],
      ['IT Modernization', `/nodes/${PROGRAM.id}`],
    ]);
    expect(text(root().querySelector('a[mat-tab-link][aria-selected="true"]'))).toBe('General');
    expect((root().querySelector('[formcontrolname="name"]') as HTMLInputElement).value).toBe('Identity platform');
    expect((root().querySelector('input[readonly]') as HTMLInputElement).value).toBe('PLAT');
    expect(await axeViolations(root().querySelector('app-node-settings-page')!)).toEqual([]);
  });

  it('saves the changes with the version it read, and says when it is stale', async () => {
    const { reply, root, text, click, type, choose } = await openSettings();
    const tab = root().querySelector('app-node-general-tab')!;
    expect(tab.querySelector('.unsaved')).toBeNull();
    await type(tab, 'objectives', 'Cloud first by 2027');
    await choose(tab, 'health', 'Off track');
    await click(tab.querySelector('button[aria-label="Remove Lucas Bernard"]'));
    expect(text(tab.querySelector('.unsaved'))).toBe('Unsaved changes');

    await click(tab.querySelector('button.save'));
    const saved = await reply((url, method) => url === `/api/v1/nodes/${PROGRAM.id}` && method === 'PUT', {
      ...PROGRAM,
      objectives: 'Cloud first by 2027',
      health: 'off_track',
      owners: [],
      version: 4,
    });
    expect(saved.request.headers.get('If-Match')).toBe('"3"');
    expect(saved.request.body).toEqual({
      name: 'IT Modernization',
      description: '',
      objectives: 'Cloud first by 2027',
      startDate: '2026-01-05',
      endDate: '2026-12-18',
      health: 'off_track',
      ownerIds: [],
    });
    expect(text(tab.querySelector('[role="status"]'))).toBe('Changes saved.');
    expect(tab.querySelector('.unsaved')).toBeNull();

    await type(tab, 'name', 'IT');
    await click(tab.querySelector('button.save'));
    const stale = await reply(
      (url, method) => url === `/api/v1/nodes/${PROGRAM.id}` && method === 'PUT',
      problem(412, 'precondition_failed'),
      412,
    );
    expect(stale.request.headers.get('If-Match')).toBe('"4"');
    expect(text(tab.querySelector('[role="alert"]'))).toBe(
      'Someone changed this item in the meantime. Reload it before trying again.',
    );
  });

  it('adds an owner found among the members', async () => {
    const { reply, settle, root, text, click } = await openSettings();
    const tab = root().querySelector('app-node-general-tab')!;
    const search = tab.querySelector('app-owner-picker input') as HTMLInputElement;
    search.focus();
    search.value = 'ines';
    search.dispatchEvent(new Event('input'));
    await new Promise((resolve) => setTimeout(resolve, 300));
    await settle();
    const found = await reply((url) => url.startsWith('/api/v1/tenant/members?'), {
      items: [
        {
          accountId: 'a3',
          email: 'ines.moreau@acme.example',
          displayName: 'Inès Moreau',
          accountType: 'internal',
          tenantRole: 'member',
          localAccount: false,
          status: 'active',
          mfaEnabled: false,
          groups: [],
          version: 1,
        },
      ],
      nextCursor: null,
    });
    expect(found.request.params.get('search')).toBe('ines');
    await click(Array.from(root().querySelectorAll('mat-option')).find((o) => o.textContent?.includes('Inès Moreau'))!);
    expect(Array.from(tab.querySelectorAll('mat-chip-row')).map((c) => text(c))).toEqual([
      'Lucas Bernard cancel',
      'Inès Moreau cancel',
    ]);
    expect(text(tab.querySelector('.unsaved'))).toBe('Unsaved changes');
    await click(tab.querySelector('button.save'));
    const saved = await reply((url, method) => url === `/api/v1/nodes/${PROGRAM.id}` && method === 'PUT', PROGRAM);
    expect((saved.request.body as { ownerIds: string[] }).ownerIds).toEqual(['a1', 'a3']);
  });

  it('adds a child among the types the matrix allows, then opens it', async () => {
    const { reply, root, text, click, dialog, type } = await openSettings();
    await click(root().querySelector('button.add-child'));
    expect(text(dialog().querySelector('h2'))).toBe('Add a child to IT Modernization');
    await click(dialog().querySelector('mat-select[formcontrolname="type"] .mat-mdc-select-trigger'));
    expect(Array.from(root().querySelectorAll('mat-option')).map((o) => text(o))).toEqual(['Program', 'Project']);
    await click(Array.from(root().querySelectorAll('mat-option')).find((o) => text(o) === 'Project')!);
    await type(dialog(), 'name', 'Identity platform');
    await type(dialog(), 'key', 'PLAT');
    await click(dialog().querySelector('button[type="submit"]'));
    const created = await reply((url) => url === '/api/v1/nodes', PROJECT, 201);
    expect((created.request.body as { parentId: string }).parentId).toBe(PROGRAM.id);
    for (let i = 0; i < 50 && !TestBed.inject(Router).url.includes(PROJECT.id); i++) {
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    await reply((url) => url === `/api/v1/nodes/${PROJECT.id}`, PROJECT);
    expect(TestBed.inject(Router).url).toBe(`/nodes/${PROJECT.id}/general`);
  });
});
