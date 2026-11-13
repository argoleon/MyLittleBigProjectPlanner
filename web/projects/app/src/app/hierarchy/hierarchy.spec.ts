/**
 * Integration tests of the hierarchy screens (L03-US01 to US05): the
 * portfolios list and the view of a node with their tree and aggregated
 * indicators, the creation dialog, a node's settings with its General and
 * Attachments tabs, the effective access card, the grant dialog, the
 * confidentiality card and the veiled rows of confidential projects, with
 * the API mocked at the HTTP level and the real translations.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { Router, provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import {
  AuthService,
  HierarchyNode,
  LocaleService,
  Node,
  NodeAccess,
  NodeAggregate,
  NodeCandidate,
  NodeLink,
  VeiledHierarchyNode,
  provideApi,
  provideMaterialDefaults,
} from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { NodeGeneralTab } from './node-general-tab';
import { NodeLinksTab } from './node-links-tab';
import { NodePage } from './node-page';
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
          { path: 'nodes/:id', pathMatch: 'full', component: NodePage },
          {
            path: 'nodes/:id',
            component: NodeSettingsPage,
            children: [
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

const isTop = (url: string, method: string) => url === '/api/v1/hierarchy' && method === 'GET';
const isUnder = (id: string) => (url: string, method: string) => url === `/api/v1/hierarchy?under=${id}` && method === 'GET';

/** The aggregate of the portfolio of mockup 05. */
const AGGREGATE: NodeAggregate = {
  projects: 6,
  confidentialProjects: 1,
  plannedMinutes: 212_400,
  capacityMinutes: 246_977,
  capacityUsedPercent: 86,
  openTickets: 1276,
  lateTickets: 17,
  progressPercent: 46,
  confidentialOpenTickets: 38,
};

/** Nothing measured, or not computed yet. */
const NOTHING: NodeAggregate = {
  projects: 0,
  confidentialProjects: 0,
  plannedMinutes: 0,
  capacityMinutes: 0,
  capacityUsedPercent: null,
  openTickets: 0,
  lateTickets: 0,
  progressPercent: null,
  confidentialOpenTickets: 0,
};

const PROGRAM_ROW: HierarchyNode = {
  id: PROGRAM.id,
  type: 'program',
  name: 'IT Modernization',
  key: null,
  health: 'at_risk',
  owners: [{ accountId: 'a1', displayName: 'Lucas Bernard' }],
  weightPercent: null,
  otherParents: [],
  hasChildren: true,
  aggregate: { ...AGGREGATE, projects: 3, plannedMinutes: 142_800, openTickets: 168, lateTickets: 11 },
};

const PROJECT_ROW: HierarchyNode = {
  id: PROJECT.id,
  type: 'project',
  name: 'Identity platform',
  key: 'PLAT',
  health: 'on_track',
  owners: [],
  weightPercent: 100,
  otherParents: [{ id: 'p2', type: 'program', name: 'Compliance' }],
  hasChildren: false,
  aggregate: { ...NOTHING, projects: 1, plannedMinutes: 37_200, openTickets: 42, progressPercent: 0 },
};

/** A project at the root, with nothing measured yet. */
const LONE_ROW: HierarchyNode = {
  ...PROJECT_ROW,
  id: 'lone',
  name: 'Steering',
  key: 'STEER',
  weightPercent: null,
  otherParents: [],
  aggregate: { ...NOTHING, projects: 1 },
};

const VEILED_ROW: VeiledHierarchyNode = { veiled: true, id: 'opaque-1', type: 'project', weightPercent: 100, plannedMinutes: 36_000 };

describe('PortfoliosPage and HierarchyTree', () => {
  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify();
  });

  const rows = (root: Element) => Array.from(root.querySelectorAll('tr[mat-row]'));
  const toggleOf = (row: Element) => row.querySelector('button.toggle') as HTMLButtonElement | null;

  it('lists the top of the hierarchy with its indicators, and loads a level when its row is expanded', async () => {
    const { harness, reply, root, text, click, http } = await setUp();
    await harness.navigateByUrl('/portfolios');
    await reply(isTop, [PROGRAM_ROW, LONE_ROW]);
    expect(text(root().querySelector('caption'))).toBe('Portfolios, programs and projects');
    expect(Array.from(root().querySelectorAll('th')).map((th) => text(th))).toEqual([
      'Name',
      'Owners',
      'Health',
      'Progress',
      'Planned load',
      'Open',
      'Late',
    ]);
    expect(rows(root()).map((row) => text(row.querySelector('.name')))).toEqual(['IT Modernization', 'Steering']);
    const program = rows(root())[0];
    const cells = Array.from(program.querySelectorAll('td'));
    expect(text(program.querySelector('ui-node-type-chip'))).toBe('Type: Program');
    expect(program.querySelector('a.name')?.getAttribute('href')).toBe(`/nodes/${PROGRAM.id}`);
    expect(text(cells[1])).toBe('Lucas Bernard');
    expect(text(cells[2])).toBe('Health: At risk');
    expect(cells[3].querySelector('mat-progress-bar')?.getAttribute('aria-valuenow')).toBe('46');
    expect(cells[3].querySelector('mat-progress-bar')?.getAttribute('aria-label')).toBe('Progress of IT Modernization');
    expect(text(cells[3])).toBe('46%');
    expect(cells.slice(4).map((cell) => text(cell))).toEqual(['2,380 h', '168', '11']);
    expect(cells[6].classList.contains('late')).toBe(true);
    // Nothing measured: no owner, no progress, zeros.
    const lone = Array.from(rows(root())[1].querySelectorAll('td'));
    expect(lone.slice(1).map((cell) => text(cell))).toEqual(['—No value', 'Health: On track', '—No value', '0 h', '0', '0']);
    expect(lone[6].classList.contains('late')).toBe(false);
    expect(toggleOf(rows(root())[1])).toBeNull();

    // The program expands with the keyboard's button; its level is loaded then.
    const toggle = toggleOf(program)!;
    expect([toggle.getAttribute('aria-expanded'), toggle.getAttribute('aria-label')]).toEqual(['false', 'Expand IT Modernization']);
    toggle.click();
    await new Promise((resolve) => setTimeout(resolve));
    TestBed.tick();
    expect(toggleOf(rows(root())[0])!.disabled).toBe(true);
    await reply(isUnder(PROGRAM.id), [PROJECT_ROW, VEILED_ROW]);
    expect(rows(root()).map((row) => text(row.querySelector('.name')))).toEqual([
      'IT Modernization',
      'Identity platform',
      'Confidential project',
      'Steering',
    ]);
    const expandedToggle = toggleOf(rows(root())[0])!;
    expect([expandedToggle.getAttribute('aria-expanded'), expandedToggle.getAttribute('aria-label')]).toEqual([
      'true',
      'Collapse IT Modernization',
    ]);
    const levels = rows(root()).map((row) => (row.querySelector('.name-cell') as HTMLElement).style.getPropertyValue('--level'));
    expect(levels).toEqual(['0', '1', '1', '0']);
    expect(rows(root()).map((row) => text(row.querySelector('.name-cell .cdk-visually-hidden')))).toEqual([
      'Level 1',
      'Level 2',
      'Level 2',
      'Level 1',
    ]);
    const project = rows(root())[1];
    expect(text(project.querySelector('.hint'))).toBe('Also under: Compliance');
    expect(project.querySelector('mat-progress-bar')?.getAttribute('aria-valuenow')).toBe('0');
    expect(text(project.querySelectorAll('td')[3])).toBe('0%');

    // The confidential project out of reach: its load, without name, link or detail.
    const veiled = rows(root())[2];
    const veiledCells = Array.from(veiled.querySelectorAll('td'));
    expect(text(veiledCells[0].querySelector('.hint'))).toBe('You do not have access to this project. Its load is counted anonymously.');
    const lock = veiled.querySelector('app-veiled-name mat-icon')!;
    expect([lock.getAttribute('role'), lock.getAttribute('aria-label')]).toEqual(['img', 'Locked']);
    expect(veiledCells.slice(1).map((cell) => text(cell))).toEqual(['—No value', '', '', '600 h', '—No value', '—No value']);
    expect(veiled.querySelector('a')).toBeNull();
    expect(veiled.querySelector('ui-health-chip')).toBeNull();
    expect(toggleOf(veiled)).toBeNull();
    expect(await axeViolations(root().querySelector('app-portfolios-page')!)).toEqual([]);

    // Folded, then expanded again: the level is read anew.
    await click(toggleOf(rows(root())[0]));
    expect(rows(root()).length).toBe(2);
    expect(http.match((request) => isUnder(PROGRAM.id)(request.urlWithParams, request.method))).toEqual([]);
    await click(toggleOf(rows(root())[0]));
    await reply(isUnder(PROGRAM.id), [PROJECT_ROW]);
    expect(rows(root()).length).toBe(3);
  });

  it('says when a level cannot be loaded, and loads it at the next try', async () => {
    const { harness, reply, root, text, click } = await setUp();
    await harness.navigateByUrl('/portfolios');
    await reply(isTop, [PROGRAM_ROW]);
    await click(toggleOf(rows(root())[0]));
    await reply(isUnder(PROGRAM.id), problem(500, 'internal'), 500);
    const failed = rows(root())[0].querySelector('.hint.failed')!;
    expect([text(failed), failed.getAttribute('role')]).toEqual(['Could not load. Try again.', 'alert']);
    expect(toggleOf(rows(root())[0])!.getAttribute('aria-expanded')).toBe('false');
    expect(rows(root()).length).toBe(1);

    await click(toggleOf(rows(root())[0]));
    await reply(isUnder(PROGRAM.id), [PROJECT_ROW]);
    expect(rows(root()).length).toBe(2);
    expect(rows(root())[0].querySelector('.hint.failed')).toBeNull();
  });

  it('formats the figures in the language of the user', async () => {
    const { harness, reply, root, text, settle } = await setUp();
    await harness.navigateByUrl('/portfolios');
    await reply(isTop, [PROGRAM_ROW]);
    const figures = () =>
      Array.from(rows(root())[0].querySelectorAll('td'))
        .slice(3)
        .map((cell) => text(cell));
    expect(figures()).toEqual(['46%', '2,380 h', '168', '11']);

    TestBed.inject(LocaleService).use('fr');
    await settle();
    expect(figures()).toEqual(['46 %', '2 380 h', '168', '11']);
    expect(text(root().querySelectorAll('th')[4])).toBe('Charge planifiée');
    expect(toggleOf(rows(root())[0])!.getAttribute('aria-label')).toBe('Développer IT Modernization');
  });

  it('says when the top cannot be loaded, and reads it again', async () => {
    const { harness, reply, root, click } = await setUp();
    await harness.navigateByUrl('/portfolios');
    await reply(isTop, problem(500, 'internal'), 500);
    expect(root().querySelector('ui-error-state')).not.toBeNull();
    await click(root().querySelector('ui-error-state button'));
    await reply(isTop, [PROGRAM_ROW]);
    expect(rows(root()).length).toBe(1);
  });

  it('only offers creation at the root to tenant admins', async () => {
    const { harness, reply, root, text } = await setUp('member');
    await harness.navigateByUrl('/portfolios');
    await reply(isTop, []);
    expect(root().querySelector('button.create')).toBeNull();
    expect(text(root().querySelector('ui-empty-state .title'))).toBe('No portfolio, program or project yet');
    expect(root().querySelector('table')).toBeNull();
  });

  it('creates a project at the root, its key in capitals, then opens its settings', async () => {
    const { harness, reply, root, text, click, dialog, type, choose } = await setUp();
    await harness.navigateByUrl('/portfolios');
    await reply(isTop, []);
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

describe('NodePage', () => {
  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify();
  });

  const nodeOf = (url: string, method: string) => url === `/api/v1/nodes/${PROGRAM.id}` && method === 'GET';
  const aggregateOf = (url: string) => url === `/api/v1/nodes/${PROGRAM.id}/aggregate`;
  const permissionsOf = (url: string) => url === `/api/v1/nodes/${PROGRAM.id}/my-permissions`;

  /** Opens the view of the program as a member with `permissions` on it. */
  async function openView(aggregate: NodeAggregate, children: object[], permissions: string[] = ['view'], tenantRole = 'member') {
    const context = await setUp(tenantRole);
    await context.harness.navigateByUrl(`/nodes/${PROGRAM.id}`);
    await context.reply(nodeOf, { ...PROGRAM, parents: [{ id: 'p0', type: 'portfolio', name: 'Digital' }] });
    await context.reply(aggregateOf, aggregate);
    await context.reply(permissionsOf, permissions);
    await context.reply(isUnder(PROGRAM.id), children);
    const page = () => context.root().querySelector('app-node-page')!;
    const indicator = (name: string) => {
      const element = page().querySelector(`.indicator.${name}`)!;
      return [context.text(element.querySelector('dt')), context.text(element.querySelector('.value')), context.text(element.querySelector('.hint'))];
    };
    return { ...context, page, indicator };
  }

  it('shows the indicators of the node over its distinct projects, then what is under it', async () => {
    const { page, indicator, text } = await openView(AGGREGATE, [PROJECT_ROW, VEILED_ROW]);
    expect(text(page().querySelector('h1'))).toBe('IT Modernization');
    expect(text(page().querySelector('.subtitle'))).toBe('Program · Owners: Lucas Bernard · Jan 5, 2026 – Dec 18, 2026');
    const crumbs = Array.from(page().querySelectorAll('nav.eyebrow a')).map((a) => [text(a), a.getAttribute('href')]);
    expect(crumbs).toEqual([
      ['Portfolios', '/portfolios'],
      ['Digital', '/nodes/p0'],
    ]);

    expect(indicator('planned')).toEqual(['Planned load', '3,540 h', '6 distinct projects']);
    expect(indicator('capacity')).toEqual(['Consumed capacity', '86%', 'of the assigned teams']);
    expect(indicator('open')).toEqual(['Open tickets', '1,276', 'including 38 on a confidential project']);
    expect(indicator('late')).toEqual(['Late tickets', '17', undefined]);
    expect(page().querySelector('.indicator.late')?.classList.contains('alert')).toBe(true);
    expect(text(page().querySelector('.note'))).toContain('a project attached to several programs is counted only once');

    expect(text(page().querySelector('caption'))).toBe('What is attached to IT Modernization');
    const names = Array.from(page().querySelectorAll('tr[mat-row]')).map((row) => text(row.querySelector('.name')));
    expect(names).toEqual(['Identity platform', 'Confidential project']);

    const links = Array.from(page().querySelectorAll('.actions a')).map((a) => [text(a), a.getAttribute('href')]);
    expect(links).toEqual([
      ['view_timeline Program Gantt', `/gantt?node=${PROGRAM.id}`],
      ['groups Team workload', `/workload?node=${PROGRAM.id}`],
      ['settings Settings', `/nodes/${PROGRAM.id}/general`],
    ]);
    // A reader adds no child.
    expect(page().querySelector('button.add-child')).toBeNull();
    expect(await axeViolations(page())).toEqual([]);
  });

  it('shows zeros and no percentage while nothing is measured', async () => {
    const { page, indicator, text, settle } = await openView({ ...NOTHING, projects: 1, confidentialProjects: 2, confidentialOpenTickets: 3 }, [
      LONE_ROW,
    ]);
    expect(indicator('planned')).toEqual(['Planned load', '0 h', '1 distinct project']);
    expect(indicator('capacity')).toEqual(['Consumed capacity', '—No value', 'No team assigned']);
    expect(indicator('open')).toEqual(['Open tickets', '0', 'including 3 on confidential projects']);
    expect(page().querySelector('.indicator.late')?.classList.contains('alert')).toBe(false);

    TestBed.inject(LocaleService).use('fr');
    await settle();
    expect(indicator('planned')).toEqual(['Charge planifiée', '0 h', '1 projet distinct']);
    expect(text(page().querySelector('.subtitle'))).toBe('Programme · Responsables : Lucas Bernard · 5 janv. 2026 – 18 déc. 2026');
  });

  it('invites those who administer the node to add its first child, which then shows in the tree', async () => {
    const { page, reply, root, text, click, dialog, type } = await openView(NOTHING, [], ['view', 'administer_node']);
    expect(page().querySelector('table')).toBeNull();
    expect(text(page().querySelector('ui-empty-state .title'))).toBe('No child yet');
    expect(text(page().querySelector('ui-empty-state .message'))).toBe('Add a child to follow its load and progress here.');
    expect(page().querySelector('button.add-child')).not.toBeNull();

    await click(page().querySelector('ui-empty-state button.add-first-child'));
    expect(text(dialog().querySelector('h2'))).toBe('Add a child to IT Modernization');
    await click(dialog().querySelector('mat-select[formcontrolname="type"] .mat-mdc-select-trigger'));
    await click(Array.from(root().querySelectorAll('mat-option')).find((o) => text(o) === 'Project')!);
    await type(dialog(), 'name', 'Identity platform');
    await type(dialog(), 'key', 'PLAT');
    await click(dialog().querySelector('button[type="submit"]'));
    const created = await reply((url) => url === '/api/v1/nodes', PROJECT, 201);
    expect((created.request.body as { parentId: string }).parentId).toBe(PROGRAM.id);
    // The view stays, and reads its tree again.
    await reply(isUnder(PROGRAM.id), [PROJECT_ROW]);
    expect(TestBed.inject(Router).url).toBe(`/nodes/${PROGRAM.id}`);
    expect(page().querySelectorAll('tr[mat-row]').length).toBe(1);
    expect(page().querySelector('ui-empty-state')).toBeNull();
  });

  it('offers no child to add to a reader, and every one to a tenant admin', async () => {
    const reader = await openView(NOTHING, []);
    expect(reader.page().querySelector('ui-empty-state')).not.toBeNull();
    expect(reader.page().querySelector('button.add-child, button.add-first-child')).toBeNull();
    TestBed.resetTestingModule();

    const admin = await openView(NOTHING, [], [], 'tenant_admin');
    expect(admin.page().querySelector('button.add-child')).not.toBeNull();
    expect(admin.page().querySelector('button.add-first-child')).not.toBeNull();
  });

  it('says when the node cannot be read', async () => {
    const { harness, reply, root } = await setUp('member');
    await harness.navigateByUrl(`/nodes/${PROGRAM.id}`);
    await reply(nodeOf, problem(404, 'not_found'), 404);
    await reply(aggregateOf, problem(404, 'not_found'), 404);
    await reply(permissionsOf, problem(404, 'not_found'), 404);
    expect(root().querySelector('app-node-page ui-error-state')).not.toBeNull();
    expect(root().querySelector('app-node-page h1.title')).toBeNull();
  });
});

describe('NodeSettingsPage and NodeGeneralTab', () => {
  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify();
  });

  async function openSettings(node: Node = PROGRAM) {
    const context = await setUp();
    await context.harness.navigateByUrl(`/nodes/${node.id}/general`);
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
      ['Identity platform', `/nodes/${PROJECT.id}`],
    ]);
    expect(text(root().querySelector('nav.eyebrow'))).toBe('Portfolios › IT Modernization › Identity platform › Settings');
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
