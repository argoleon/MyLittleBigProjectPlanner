/**
 * Integration tests of the hierarchy screens (L03-US01 to US04): the
 * portfolios list, the creation dialog, a node's settings with its General
 * and Attachments tabs, the effective access card, the grant dialog, the
 * confidentiality card and the veiled rows of confidential projects, with
 * the API mocked at the HTTP level and the real translations.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { Router, provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { AuthService, Node, NodeAccess, NodeCandidate, NodeLink, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { NodeGeneralTab } from './node-general-tab';
import { NodeLinksTab } from './node-links-tab';
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
              { path: 'links', component: NodeLinksTab },
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

  it('shows a confidential project out of reach as a veiled row, without name or link', async () => {
    const { harness, reply, root, text } = await setUp('member');
    await harness.navigateByUrl('/portfolios');
    await reply(isList, {
      items: [
        PROGRAM,
        { veiled: true, id: 'opaque-1', type: 'project', parents: [{ id: PROGRAM.id, type: 'program', name: 'IT Modernization' }] },
      ],
      nextCursor: null,
    });
    const veiled = root().querySelectorAll('tr[mat-row]')[1];
    const cells = Array.from(veiled.querySelectorAll('td'));
    expect(text(cells[0].querySelector('app-veiled-name .name'))).toBe('Confidential project');
    expect(text(cells[0].querySelector('.meta'))).toBe('Type: Project You do not have access.');
    expect(cells.slice(1).map((cell) => text(cell))).toEqual(['IT Modernization', '', '', '']);
    const lock = veiled.querySelector('app-veiled-name mat-icon')!;
    expect([lock.getAttribute('role'), lock.getAttribute('aria-label')]).toEqual(['img', 'Locked']);
    expect(veiled.querySelector('a')).toBeNull();
    expect(veiled.querySelector('ui-health-chip')).toBeNull();
    expect(await axeViolations(root().querySelector('app-portfolios-page')!)).toEqual([]);
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

describe('NodeLinksTab', () => {
  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify();
  });

  const MODERNIZATION: NodeLink = {
    id: PROGRAM.id,
    type: 'program',
    name: 'IT Modernization',
    key: null,
    weightPercent: 100,
    parents: [{ id: 'p0', type: 'portfolio', name: 'Digital' }],
  };
  const COMPLIANCE: NodeLink = { ...MODERNIZATION, id: 'p2', name: 'Compliance', weightPercent: 40 };
  const SUB_PROJECT: NodeLink = {
    id: 'c1',
    type: 'project',
    name: 'Cyber permissions',
    key: 'CYBP',
    weightPercent: 100,
    parents: [{ id: PROJECT.id, type: 'project', name: 'Identity platform' }],
  };
  const parentsOf = (url: string, method: string) => url === `/api/v1/nodes/${PROJECT.id}/parents` && method === 'GET';
  const childrenOf = (url: string) => url === `/api/v1/nodes/${PROJECT.id}/children`;
  const nodeRead = (url: string, method: string) => url === `/api/v1/nodes/${PROJECT.id}` && method === 'GET';
  const linkTo = (parent: string) => (url: string, method: string) =>
    url === `/api/v1/nodes/${PROJECT.id}/parents/${parent}` && method === 'PUT';

  const permissionsOf = (url: string) => url === `/api/v1/nodes/${PROJECT.id}/my-permissions`;
  const accessOf = (url: string, method: string) => url === `/api/v1/nodes/${PROJECT.id}/access` && method === 'GET';

  /** Opens the tab as a member with `permissions` on the project (a reader, by default). */
  async function openLinks(
    parents: NodeLink[] = [MODERNIZATION, COMPLIANCE],
    children: NodeLink[] = [SUB_PROJECT],
    permissions: string[] = ['view'],
  ) {
    const context = await setUp();
    await context.harness.navigateByUrl(`/nodes/${PROJECT.id}/links`);
    await context.reply(nodeRead, PROJECT);
    await context.reply(parentsOf, parents);
    await context.reply(childrenOf, children);
    await context.reply(permissionsOf, permissions);
    const tab = () => context.root().querySelector('app-node-links-tab')!;
    return { ...context, tab };
  }

  async function search(context: Awaited<ReturnType<typeof openLinks>>, query: string, found: NodeCandidate[]) {
    const input = context.tab().querySelector('#parent-search input') as HTMLInputElement;
    input.value = query;
    input.dispatchEvent(new Event('input'));
    await new Promise((resolve) => setTimeout(resolve, 300));
    await context.settle();
    return context.reply((url) => url.startsWith(`/api/v1/nodes/${PROJECT.id}/parent-candidates?`), found);
  }

  it('shows the parents with their weighting and the children to open', async () => {
    const { root, tab, text } = await openLinks();
    expect(text(root().querySelector('a[mat-tab-link][aria-selected="true"]'))).toBe('Attachments and permissions');
    expect(text(tab().querySelector('.hint'))).toBe(
      'A project can be attached to portfolios, programs or other projects.',
    );
    const parents = Array.from(tab().querySelectorAll('.parents .row'));
    expect(parents.map((row) => text(row.querySelector('.text')))).toEqual([
      'IT Modernization Program · Digital',
      'Compliance Program · Digital',
    ]);
    expect(parents.map((row) => (row.querySelector('app-weight-field input') as HTMLInputElement).value)).toEqual([
      '100',
      '40',
    ]);
    expect(parents[1].querySelector('button.unlink')?.getAttribute('aria-label')).toBe('Detach from Compliance');
    expect(parents[0].querySelector('a.name')?.getAttribute('href')).toBe(`/nodes/${PROGRAM.id}`);
    const child = tab().querySelector('.children .row')!;
    expect(text(child.querySelector('.text'))).toBe('Cyber permissions Project · CYBP');
    expect(child.querySelector('a')?.getAttribute('href')).toBe('/nodes/c1');
    expect(child.querySelector('a')?.getAttribute('aria-label')).toBe('Open Cyber permissions');
    expect(await axeViolations(root().querySelector('app-node-settings-page')!)).toEqual([]);
  });

  it('says when there is no parent nor child', async () => {
    const { tab, text } = await openLinks([], []);
    expect(Array.from(tab().querySelectorAll('.empty')).map((p) => text(p))).toEqual([
      'At the root: no parent.',
      'No children yet.',
    ]);
    expect(tab().querySelector('.note')).toBeNull();
  });

  it('saves a new weighting of a parent link', async () => {
    const { reply, tab, text, settle } = await openLinks();
    const input = tab().querySelectorAll('app-weight-field input')[1] as HTMLInputElement;
    input.value = '60';
    input.dispatchEvent(new Event('input'));
    input.dispatchEvent(new Event('change'));
    await settle();
    const saved = await reply(linkTo('p2'), { ...COMPLIANCE, weightPercent: 60 });
    expect(saved.request.body).toEqual({ weightPercent: 60 });
    await reply(parentsOf, [MODERNIZATION, { ...COMPLIANCE, weightPercent: 60 }]);
    await reply(nodeRead, PROJECT);
    expect(text(tab().querySelector('[role="status"]'))).toBe('Weighting under Compliance saved.');
  });

  it('adds a parent among the allowed candidates, the refused ones saying why', async () => {
    const context = await openLinks([MODERNIZATION]);
    const { reply, root, tab, text, click } = context;
    const add = tab().querySelector('button.add-parent') as HTMLButtonElement;
    expect(add.getAttribute('aria-expanded')).toBe('false');
    await click(add);
    expect(add.getAttribute('aria-expanded')).toBe('true');
    await reply((url) => url === `/api/v1/nodes/${PROJECT.id}/parent-candidates?limit=20`, []);
    expect(text(tab().querySelector('#parent-search .empty'))).toBe('No matching node.');
    expect(document.activeElement).toBe(tab().querySelector('#parent-search input'));

    const found = await search(context, 'cyber', [
      { id: 'p3', type: 'program', name: 'Cybersecurity', key: null, parents: [], refusal: null },
      { id: 'c1', type: 'project', name: 'Cyber permissions', key: 'CYBP', parents: [], refusal: 'cycle' },
    ]);
    expect(found.request.params.get('search')).toBe('cyber');
    const items = Array.from(tab().querySelectorAll('.candidates li'));
    expect(items.map((li) => text(li.querySelector('.text')))).toEqual([
      'Cybersecurity Program · attachment allowed',
      'Cyber permissions Impossible: it is a descendant of this node, the link would create a cycle',
    ]);
    expect(items[0].querySelector('button')?.textContent?.replace(/\s+/g, ' ')).toContain('Cybersecurity Program');
    expect(items[1].querySelector('button')).toBeNull();
    expect(await axeViolations(root().querySelector('app-node-links-tab')!)).toEqual([]);

    await click(items[0].querySelector('button'));
    const linked = await reply(linkTo('p3'), { ...MODERNIZATION, id: 'p3', name: 'Cybersecurity', parents: [] }, 201);
    expect(linked.request.body).toEqual({ weightPercent: 100 });
    await reply(parentsOf, [MODERNIZATION, { ...MODERNIZATION, id: 'p3', name: 'Cybersecurity', parents: [] }]);
    await reply(nodeRead, PROJECT);
    await reply((url) => url.startsWith(`/api/v1/nodes/${PROJECT.id}/parent-candidates?`), [
      { id: 'p3', type: 'program', name: 'Cybersecurity', key: null, parents: [], refusal: 'already_parent' },
    ]);
    expect(text(tab().querySelector('[role="status"]'))).toBe('Attached under Cybersecurity.');
    expect(text(tab().querySelector('.candidates .reason'))).toBe('Already a parent');

    await click(tab().querySelector('#parent-search button[aria-label="Close the search"]'));
    expect(tab().querySelector('#parent-search')).toBeNull();
    expect(document.activeElement).toBe(add);
  });

  it('says why a link is refused', async () => {
    const context = await openLinks([MODERNIZATION]);
    const { reply, tab, text, click } = context;
    await click(tab().querySelector('button.add-parent'));
    await reply((url) => url.startsWith(`/api/v1/nodes/${PROJECT.id}/parent-candidates?`), [
      { id: 'p3', type: 'program', name: 'Cybersecurity', key: null, parents: [], refusal: null },
    ]);
    await click(tab().querySelector('.candidates button'));
    await reply(linkTo('p3'), problem(409, 'node.cycle'), 409);
    await reply(parentsOf, [MODERNIZATION]);
    expect(text(tab().querySelector('[role="alert"]'))).toBe('This link would make the node its own ancestor.');
  });

  it('detaches the node from a parent', async () => {
    const { reply, tab, text, click } = await openLinks();
    await click(tab().querySelector('button[aria-label="Detach from Compliance"]'));
    const removed = await reply(
      (url, method) => url === `/api/v1/nodes/${PROJECT.id}/parents/p2` && method === 'DELETE',
      null,
      204,
    );
    expect(removed.request.method).toBe('DELETE');
    await reply(parentsOf, [MODERNIZATION]);
    await reply(nodeRead, PROJECT);
    expect(text(tab().querySelector('[role="status"]'))).toBe('Detached from Compliance.');
    expect(tab().querySelectorAll('.parents .row').length).toBe(1);
    expect(document.activeElement).toBe(tab().querySelector('button.add-parent'));
  });

  it('shows a confidential child out of reach veiled, with nothing to open', async () => {
    const { tab, text } = await openLinks(
      [MODERNIZATION],
      [SUB_PROJECT, { veiled: true, id: 'opaque-1', type: 'project', parents: [] } as unknown as NodeLink],
    );
    const rows = Array.from(tab().querySelectorAll('.children .row'));
    expect(text(rows[0].querySelector('.text'))).toBe('Cyber permissions Project · CYBP');
    expect(text(rows[1].querySelector('.name'))).toBe('Confidential project');
    expect(text(rows[1].querySelector('.meta'))).toBe('You do not have access.');
    expect(rows[1].querySelector('a')).toBeNull();
    const lock = rows[1].querySelector('mat-icon')!;
    expect([lock.textContent, lock.getAttribute('role'), lock.getAttribute('aria-label')]).toEqual(['lock', 'img', 'Locked']);
    expect(await axeViolations(tab())).toEqual([]);
  });

  describe('confidentiality', () => {
    const confidentiality = (url: string, method: string) =>
      url === `/api/v1/nodes/${PROJECT.id}/confidentiality` && method === 'PUT';

    it('makes the project confidential with the version it read, and says when it is refused', async () => {
      const { reply, root, tab, text, click } = await openLinks([MODERNIZATION], [], ['view', 'administer_node']);
      const card = () => tab().querySelector('app-node-confidentiality-card')!;
      expect(text(card().querySelector('h2'))).toBe('Confidentiality');
      const toggle = () => card().querySelector('button[role="switch"]') as HTMLButtonElement;
      expect(toggle().getAttribute('aria-checked')).toBe('false');
      expect(toggle().getAttribute('aria-labelledby')).toBe('confidentiality-label');
      expect(text(card().querySelector('#confidentiality-label'))).toBe('Confidential project');
      expect(toggle().getAttribute('aria-describedby')).toBe('confidentiality-description');
      expect(await axeViolations(root().querySelector('app-node-settings-page')!)).toEqual([]);
      // Only those who manage the members see who has access.
      expect(tab().querySelector('app-node-access-card')).toBeNull();

      await click(toggle());
      const saved = await reply(confidentiality, { ...PROJECT, confidential: true, version: 2 });
      expect(saved.request.headers.get('If-Match')).toBe('"1"');
      expect(saved.request.body).toEqual({ confidential: true });
      expect(toggle().getAttribute('aria-checked')).toBe('true');
      expect(text(card().querySelector('[role="status"]'))).toBe('The project is now confidential.');

      // Opening it again is refused: the switch stays on.
      await click(toggle());
      const refused = await reply(confidentiality, problem(409, 'node.confidential_lockout'), 409);
      expect(refused.request.headers.get('If-Match')).toBe('"2"');
      expect(refused.request.body).toEqual({ confidential: false });
      expect(toggle().getAttribute('aria-checked')).toBe('true');
      expect(text(card().querySelector('[role="alert"]'))).toBe(
        'You would lose your own access: ask for a role granted on the project itself first.',
      );
    });

    it('reads the access again once the project is confidential', async () => {
      const { reply, tab, click } = await openLinks([MODERNIZATION], [], ['view', 'administer_node', 'manage_members']);
      await reply(accessOf, []);
      await click(tab().querySelector('app-node-confidentiality-card button[role="switch"]'));
      await reply(confidentiality, { ...PROJECT, confidential: true, version: 2 });
      // Inherited roles no longer reach it.
      await reply(accessOf, []);
    });

    it('is for projects only, and for those who administer them', async () => {
      const reader = await openLinks([MODERNIZATION], []);
      expect(reader.tab().querySelector('app-node-confidentiality-card')).toBeNull();
      TestBed.resetTestingModule();

      const context = await setUp();
      await context.harness.navigateByUrl(`/nodes/${PROGRAM.id}/links`);
      await context.reply((url) => url === `/api/v1/nodes/${PROGRAM.id}`, PROGRAM);
      await context.reply((url) => url === `/api/v1/nodes/${PROGRAM.id}/parents`, []);
      await context.reply((url) => url === `/api/v1/nodes/${PROGRAM.id}/children`, []);
      await context.reply((url) => url === `/api/v1/nodes/${PROGRAM.id}/my-permissions`, ['view', 'administer_node']);
      expect(context.root().querySelector('app-node-links-tab')).not.toBeNull();
      expect(context.root().querySelector('app-node-confidentiality-card')).toBeNull();
    });
  });

  describe('effective access', () => {
    const READER = { id: 'r1', label: 'Reader', labelTranslations: { fr: 'Lecteur', en: 'Reader' } };
    const MANAGER = { id: 'r3', label: 'Project manager', labelTranslations: { en: 'Project manager' } };
    const DIRECT: NodeAccess = {
      grantId: 'g1',
      holder: { kind: 'member', id: 'a2', name: 'Camille Martin' },
      role: MANAGER,
      origin: { id: PROJECT.id, type: 'project', name: 'Identity platform' },
      direct: true,
    };
    const FROM_PORTFOLIO: NodeAccess = {
      grantId: 'g2',
      holder: { kind: 'group', id: 'gr1', name: 'Managers' },
      role: MANAGER,
      origin: { id: 'p0', type: 'portfolio', name: 'Digital' },
      direct: false,
    };
    const FROM_PROGRAM: NodeAccess = {
      grantId: 'g3',
      holder: { kind: 'group', id: 'gr2', name: 'Compliance team' },
      role: READER,
      origin: { id: 'p2', type: 'program', name: 'Compliance' },
      direct: false,
    };
    const candidatesOf = (url: string) => url.startsWith(`/api/v1/nodes/${PROJECT.id}/grant-candidates`);
    const rolesOf = (url: string) => url === `/api/v1/nodes/${PROJECT.id}/grantable-roles`;
    const grants = (url: string, method: string) => url === `/api/v1/nodes/${PROJECT.id}/grants` && method === 'POST';

    async function openAccess(access: NodeAccess[] = [DIRECT, FROM_PORTFOLIO, FROM_PROGRAM]) {
      const context = await openLinks([MODERNIZATION], [], ['view', 'manage_members']);
      await context.reply(accessOf, access);
      const card = () => context.root().querySelector('app-node-access-card')!;
      return { ...context, card };
    }

    /** Opens the grant dialog, with its first candidates and the roles to grant. */
    async function openDialog(context: Awaited<ReturnType<typeof openAccess>>) {
      await context.click(context.card().querySelector('button.add-grant'));
      await context.reply(candidatesOf, []);
      await context.reply(rolesOf, [READER, MANAGER]);
      return context.dialog();
    }

    it('lists who has access, with the role and where it comes from', async () => {
      const { root, card, text } = await openAccess();
      expect(text(card().querySelector('h2'))).toBe('Effective access');
      const rows = Array.from(card().querySelectorAll('.row'));
      expect(rows.map((row) => text(row.querySelector('.text')))).toEqual([
        'Member: Camille Martin Direct role on this project',
        'Group: Managers Inherited from the portfolio Digital',
        'Group: Compliance team Inherited from the program Compliance',
      ]);
      expect(rows.map((row) => text(row.querySelector('ui-role-chip')))).toEqual([
        'Role: Project manager',
        'Role: Project manager',
        'Role: Reader',
      ]);
      expect(rows.map((row) => row.querySelector('.chip')?.getAttribute('data-origin'))).toEqual([
        'direct',
        'inherited',
        'inherited',
      ]);
      expect(rows.map((row) => row.querySelector('mat-icon')?.textContent)).toEqual(['person', 'group', 'group']);
      // Only a direct role is revoked here.
      expect(rows.map((row) => row.querySelector('button.revoke')?.getAttribute('aria-label') ?? null)).toEqual([
        'Revoke the role Project manager of Camille Martin',
        null,
        null,
      ]);
      expect(card().querySelector('button.add-grant')?.getAttribute('aria-label')).toBe('Add a role');
      expect(await axeViolations(root().querySelector('app-node-settings-page')!)).toEqual([]);
    });

    it('does not name an ancestor the user has no access to', async () => {
      const { card, text } = await openAccess([{ ...FROM_PROGRAM, origin: null }]);
      expect(text(card().querySelector('.row .text'))).toBe(
        'Group: Compliance team Inherited from a node you do not have access to',
      );
    });

    it('says when nobody has a role, and stays hidden from those who do not manage the members', async () => {
      const { card, text } = await openAccess([]);
      expect(text(card().querySelector('.empty'))).toBe(
        'Nobody has a role here yet. Organization admins see every node.',
      );
      TestBed.resetTestingModule();
      const reader = await openLinks([MODERNIZATION], []);
      expect(reader.root().querySelector('app-node-access-card')).toBeNull();
    });

    it('grants a role to a group found by its name', async () => {
      const context = await openAccess([]);
      const { reply, settle, root, card, text, click } = context;
      const dialog = await openDialog(context);
      expect(text(dialog.querySelector('h2'))).toBe('Grant a role on Identity platform');

      // Typed, not chosen: nothing is sent.
      const holder = dialog.querySelector('[formcontrolname="holder"]') as HTMLInputElement;
      holder.focus();
      holder.value = 'man';
      holder.dispatchEvent(new Event('input'));
      await new Promise((resolve) => setTimeout(resolve, 300));
      await settle();
      const found = await reply(candidatesOf, [
        { kind: 'group', id: 'gr1', name: 'Managers', email: null },
        { kind: 'member', id: 'a4', name: 'Manon Petit', email: 'manon.petit@acme.example' },
      ]);
      expect(found.request.params.get('search')).toBe('man');
      const options = Array.from(root().querySelectorAll('mat-option'));
      expect(options.map((o) => text(o.querySelector('.mdc-list-item__primary-text')))).toEqual([
        'Group: Managers',
        'Member: Manon Petit · manon.petit@acme.example',
      ]);
      expect(options.map((o) => o.querySelector('mat-icon')?.textContent)).toEqual(['group', 'person']);
      await click(dialog.querySelector('button[type="submit"]'));
      expect(Array.from(dialog.querySelectorAll('mat-error')).map((e) => text(e))).toEqual([
        'Choose a member or a group in the list.',
        'This field is required.',
      ]);

      // Typing again reopens the list; the group is chosen in it.
      holder.focus();
      holder.value = 'mana';
      holder.dispatchEvent(new Event('input'));
      await new Promise((resolve) => setTimeout(resolve, 300));
      await settle();
      await reply(candidatesOf, [{ kind: 'group', id: 'gr1', name: 'Managers', email: null }]);
      await click(root().querySelector('mat-option'));
      expect(holder.value).toBe('Managers');
      await click(dialog.querySelector('mat-select[formcontrolname="roleId"] .mat-mdc-select-trigger'));
      expect(Array.from(root().querySelectorAll('mat-option')).map((o) => text(o))).toEqual(['Reader', 'Project manager']);
      await click(Array.from(root().querySelectorAll('mat-option')).find((o) => text(o) === 'Reader')!);
      expect(await axeViolations(dialog)).toEqual([]);

      await click(dialog.querySelector('button[type="submit"]'));
      const granted: NodeAccess = {
        grantId: 'g9',
        holder: { kind: 'group', id: 'gr1', name: 'Managers' },
        role: READER,
        origin: { id: PROJECT.id, type: 'project', name: 'Identity platform' },
        direct: true,
      };
      const sent = await reply(grants, granted, 201);
      expect(sent.request.body).toEqual({ roleId: 'r1', groupId: 'gr1' });
      await reply(accessOf, [granted]);
      expect(root().querySelector('mat-dialog-container')).toBeNull();
      expect(text(card().querySelector('[role="status"]'))).toBe('Role Reader granted to Managers.');
      expect(card().querySelectorAll('.row').length).toBe(1);
    });

    it('grants to a member, and says when they already have the role', async () => {
      const context = await openAccess([DIRECT]);
      const { reply, settle, root, text, click } = context;
      const dialog = await openDialog(context);
      const holder = dialog.querySelector('[formcontrolname="holder"]') as HTMLInputElement;
      holder.focus();
      holder.value = 'camille';
      holder.dispatchEvent(new Event('input'));
      await new Promise((resolve) => setTimeout(resolve, 300));
      await settle();
      await reply(candidatesOf, [{ kind: 'member', id: 'a2', name: 'Camille Martin', email: 'camille@acme.example' }]);
      await click(root().querySelector('mat-option'));
      await click(dialog.querySelector('mat-select[formcontrolname="roleId"] .mat-mdc-select-trigger'));
      await click(Array.from(root().querySelectorAll('mat-option')).find((o) => text(o) === 'Project manager')!);
      await click(dialog.querySelector('button[type="submit"]'));
      const refused = await reply(grants, problem(409, 'node.grant_exists'), 409);
      expect(refused.request.body).toEqual({ roleId: 'r3', accountId: 'a2' });
      expect(text(dialog.querySelector('[role="alert"]'))).toBe('They already have this role on this node.');
      expect(root().querySelector('mat-dialog-container')).not.toBeNull();
    });

    it('revokes a direct role', async () => {
      const { reply, card, text, click } = await openAccess();
      await click(card().querySelector('button.revoke'));
      const revoked = await reply(
        (url, method) => url === `/api/v1/nodes/${PROJECT.id}/grants/g1` && method === 'DELETE',
        null,
        204,
      );
      expect(revoked.request.method).toBe('DELETE');
      await reply(accessOf, [FROM_PORTFOLIO, FROM_PROGRAM]);
      expect(text(card().querySelector('[role="status"]'))).toBe('Role Project manager revoked from Camille Martin.');
      expect(card().querySelectorAll('.row').length).toBe(2);
      expect(document.activeElement).toBe(card().querySelector('button.add-grant'));
    });
  });
});
