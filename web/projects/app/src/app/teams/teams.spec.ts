/**
 * Tests of the teams (L07-US02), the API mocked at the HTTP level and the
 * real translations: the Workload tabs, the teams list with create, rename
 * and delete, the team page with its members added and removed, read-only
 * for members who do not manage resources, errors, accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { AuthService, Resource, Session, Team, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { WorkloadPage } from '../workload/workload-page';
import { TeamPage } from './team-page';
import { TeamsPage } from './teams-page';

const PLATFORM: Team = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7c01',
  name: 'Platform',
  nameTranslations: { en: 'Platform', fr: 'Plateforme' },
  description: 'Identity services',
  memberCount: 2,
  version: 3,
};
const RUN: Team = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7c02',
  name: 'Run',
  nameTranslations: { fr: 'Run' },
  memberCount: 0,
  version: 1,
};
const ALICE: Resource = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b01',
  kind: 'person',
  displayName: 'Alice Martin',
  jobTitle: 'Dev back',
  accountId: null,
  formerMember: false,
  active: true,
  teams: [{ id: PLATFORM.id, name: 'Platform' }],
  site: null,
  version: 1,
};
const BOB: Resource = { ...ALICE, id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b02', displayName: 'Bob Durand', jobTitle: null, active: false };
const CAROL: Resource = { ...ALICE, id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b03', displayName: 'Carol Leroy', teams: [] };

function problem(status: number, code: string, errors: object[] = []) {
  return { type: `urn:mlbpp:problem:${code}`, title: code, status, code, errors };
}

function setup(routes = [] as Parameters<typeof provideRouter>[0]) {
  localStorage.clear();
  TestBed.configureTestingModule({
    providers: [provideRouter(routes), provideApi(), provideHttpClientTesting(), provideI18nTesting(), provideMaterialDefaults()],
  });
  const http = TestBed.inject(HttpTestingController);
  const settle = async () => {
    for (let i = 0; i < 3; i++) {
      await new Promise((resolve) => setTimeout(resolve, 10));
      TestBed.tick();
    }
  };
  const reply = async (match: (url: string, method: string) => boolean, body: object | null, status = 200): Promise<TestRequest> => {
    let request: TestRequest | undefined;
    for (let i = 0; i < 50 && !request; i++) {
      [request] = http.match((r) => match(r.urlWithParams, r.method));
      if (!request) await new Promise((resolve) => setTimeout(resolve, 20));
    }
    if (!request) throw new Error(`no matching request among ${http.match(() => true).map((r) => `${r.request.method} ${r.request.urlWithParams}`).join(', ') || 'none'}`);
    request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error' });
    await settle();
    return request;
  };
  const root = () => document.body;
  const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
  const click = async (element: Element | null) => {
    if (!element) throw new Error('nothing to click');
    (element as HTMLElement).click();
    await settle();
  };
  const type = async (input: Element | null, value: string) => {
    const field = input as HTMLInputElement;
    field.value = value;
    field.dispatchEvent(new Event('input'));
    field.dispatchEvent(new Event('focusin'));
    await settle();
  };
  /** The dialog opened last: closed ones may linger while they animate out. */
  const dialog = () => Array.from(root().querySelectorAll('mat-dialog-container')).at(-1) ?? root();
  /** Lets a closing dialog finish its animation and hand its result. */
  const closed = async () => {
    await new Promise((resolve) => setTimeout(resolve, 200));
    await settle();
  };
  return { http, settle, reply, root, text, click, type, dialog, closed };
}

async function signIn(kit: ReturnType<typeof setup>, role: Session['account']['tenantRole']) {
  const loaded = TestBed.inject(AuthService).load();
  await kit.reply((url) => url === '/api/v1/auth/session', {
    account: { id: 'me', email: 'me@acme.example', displayName: 'Me', tenantRole: role },
    csrfToken: 'csrf',
    expiresAt: '2026-12-01T00:00:00Z',
    stage: 'complete',
  });
  await loaded;
}

const isTeams = (url: string, method: string) => url === '/api/v1/teams' && method === 'GET';

describe('WorkloadPage', () => {
  it('shows the Heatmap, Resources, Teams and Calendars as routed tabs', async () => {
    const kit = setup([
      {
        path: 'workload',
        component: WorkloadPage,
        children: [
          { path: 'resources', children: [] },
          { path: 'teams', component: TeamsPage },
        ],
      },
    ]);
    const harness = await RouterTestingHarness.create();
    await harness.navigateByUrl('/workload/teams');
    await kit.reply(isTeams, { items: [], canManage: false });
    expect(kit.text(kit.root().querySelector('h1'))).toBe('Workload');
    const tabs = Array.from(kit.root().querySelectorAll('a[mat-tab-link]'));
    expect(tabs.map((tab) => kit.text(tab))).toEqual(['Heatmap', 'Resources', 'Teams', 'Calendars']);
    expect(tabs.map((tab) => tab.getAttribute('aria-selected'))).toEqual(['false', 'false', 'true', 'false']);
    expect(await axeViolations(harness.routeNativeElement!.parentElement!)).toEqual([]);
    TestBed.inject(HttpTestingController).verify();
  });
});

describe('TeamsPage', () => {
  async function open(items: readonly Team[] | null, canManage = true) {
    const kit = setup();
    const fixture = TestBed.createComponent(TeamsPage);
    await kit.settle();
    if (items) await kit.reply(isTeams, { items, canManage });
    else await kit.reply(isTeams, problem(500, 'internal'), 500);
    const rows = () => Array.from(kit.root().querySelectorAll('tr[mat-row]'));
    return { ...kit, fixture, rows };
  }

  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify();
  });

  it('lists the teams, each opening its page, with their size', async () => {
    const { rows, text, root, fixture } = await open([PLATFORM, RUN]);
    expect(
      rows().map((row) => [text(row.querySelector('.team-link')), text(row.querySelector('.secondary')), text(row.querySelector('.count'))]),
    ).toEqual([
      ['Platform', 'Identity services', '2'],
      ['Run', undefined, '0'],
    ]);
    expect(root().querySelector('a.team-link')?.getAttribute('href')).toBe(`/workload/teams/${PLATFORM.id}`);
    expect(Array.from(root().querySelectorAll('td button')).map((b) => b.getAttribute('aria-label'))).toEqual([
      'Edit Platform',
      'Delete Platform',
      'Edit Run',
      'Delete Run',
    ]);
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
  });

  it('is read-only for a member who does not manage resources', async () => {
    const { rows, root } = await open([PLATFORM], false);
    expect(rows().length).toBe(1);
    expect(root().querySelector('button.new')).toBeNull();
    expect(root().querySelector('td button')).toBeNull();
  });

  it('says when there is no team yet, and when the teams cannot be read', async () => {
    const empty = await open([]);
    expect(empty.root().querySelector('ui-empty-state')?.textContent).toContain('No team yet');
    TestBed.resetTestingModule();
    const failed = await open(null);
    expect(failed.root().querySelector('ui-error-state')).not.toBeNull();
  });

  it('creates a team with its names, and shows a name already taken', async () => {
    const { root, click, type, reply, dialog, closed, text } = await open([RUN]);
    await click(root().querySelector('button.new'));
    expect(text(dialog().querySelector('h2'))).toBe('New team');
    expect(await axeViolations(dialog())).toEqual([]);
    // The name is required.
    await click(dialog().querySelector('button[type="submit"]'));
    expect(dialog().querySelector('mat-error')).not.toBeNull();

    const inputs = dialog().querySelectorAll('ui-translatable-input input');
    await type(inputs[0], 'Plateforme');
    await type(inputs[1], 'Platform');
    await type(dialog().querySelector('textarea'), ' Identity services ');
    await click(dialog().querySelector('button[type="submit"]'));
    const refused = await reply(
      (url, method) => url === '/api/v1/teams' && method === 'POST',
      problem(409, 'team.name_taken', [{ field: 'nameTranslations', code: 'taken' }]),
      409,
    );
    expect(refused.request.body).toEqual({
      nameTranslations: { fr: 'Plateforme', en: 'Platform' },
      description: 'Identity services',
    });
    expect(text(dialog().querySelector('mat-error'))).toBe('This value is already used.');

    await click(dialog().querySelector('button[type="submit"]'));
    const created = await reply((url, method) => url === '/api/v1/teams' && method === 'POST', PLATFORM, 201);
    expect(created.request.headers.get('Idempotency-Key')).toBeTruthy();
    await closed();
    await reply(isTeams, { items: [PLATFORM, RUN], canManage: true });
    expect(text(root().querySelector('[role="status"]'))).toBe('Platform was created.');
  });

  it('renames a team with its version, and deletes one after confirmation', async () => {
    const { root, click, type, reply, dialog, closed, text } = await open([PLATFORM]);
    await click(root().querySelector(`#team-edit-${PLATFORM.id}`));
    expect(text(dialog().querySelector('h2'))).toBe('Edit Platform');
    await type(dialog().querySelectorAll('ui-translatable-input input')[1], 'Core platform');
    await click(dialog().querySelector('button[type="submit"]'));
    const saved = await reply((url, method) => url === `/api/v1/teams/${PLATFORM.id}` && method === 'PUT', {
      ...PLATFORM,
      name: 'Core platform',
      version: 4,
    });
    expect(saved.request.headers.get('If-Match')).toBe('"3"');
    expect(saved.request.body).toEqual({
      nameTranslations: { fr: 'Plateforme', en: 'Core platform' },
      description: 'Identity services',
    });
    await closed();
    await reply(isTeams, { items: [{ ...PLATFORM, name: 'Core platform', version: 4 }], canManage: true });
    expect(text(root().querySelector('[role="status"]'))).toBe('Core platform was saved.');

    await click(root().querySelector(`#team-delete-${PLATFORM.id}`));
    expect(text(dialog().querySelector('h2'))).toBe('Delete Core platform?');
    await click(Array.from(dialog().querySelectorAll('button')).find((b) => text(b) === 'Delete') ?? null);
    await closed();
    const deleted = await reply((url, method) => url === `/api/v1/teams/${PLATFORM.id}` && method === 'DELETE', null, 204);
    expect(deleted.request.headers.get('If-Match')).toBe('"4"');
    await reply(isTeams, { items: [], canManage: true });
    expect(text(root().querySelector('[role="status"]'))).toBe('Core platform was deleted.');
  });
});

describe('TeamPage', () => {
  const isMembers = (url: string) => url.startsWith('/api/v1/resources?') && url.includes(`team=${PLATFORM.id}`);

  async function open(role: Session['account']['tenantRole'], members: readonly Resource[] = [ALICE, BOB]) {
    const kit = setup();
    await signIn(kit, role);
    const fixture = TestBed.createComponent(TeamPage);
    fixture.componentRef.setInput('id', PLATFORM.id);
    await kit.settle();
    await kit.reply((url) => url === `/api/v1/teams/${PLATFORM.id}`, PLATFORM);
    const list = await kit.reply(isMembers, { items: members, nextCursor: null, canManage: true });
    const rows = () => Array.from(kit.root().querySelectorAll('tr[mat-row]'));
    return { ...kit, fixture, list, rows };
  }

  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify();
  });

  it('shows the team and all its resources, active or not', async () => {
    const { root, text, rows, list, fixture, http } = await open('resource_manager');
    // Only the team's resources were asked for, inactive ones included.
    expect(http.match((r) => r.url === '/api/v1/resources').length).toBe(0);
    expect(list.request.params.get('active')).toBeNull();
    expect(text(root().querySelector('h1'))).toBe('Platform');
    expect(text(root().querySelector('.description'))).toBe('Identity services');
    expect(text(root().querySelector('h2'))).toBe('Resources (2)');
    expect(rows().map((row) => text(row.querySelector('.name')))).toEqual(['Alice Martin', 'Bob Durand']);
    expect(text(rows()[1].querySelector('ui-resource-state-chip'))).toBe('Status: Inactive');
    expect(root().querySelector('.breadcrumb a')?.getAttribute('href')).toBe('/workload/teams');
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
  });

  it('adds a resource found by name, and removes one', async () => {
    const { root, click, type, reply, dialog, closed, text, rows, settle, fixture } = await open('tenant_admin');
    await click((fixture.nativeElement as HTMLElement).querySelector('#team-add'));
    const first = await reply((url) => url.startsWith('/api/v1/resources?') && url.includes('active=true'), {
      items: [ALICE, CAROL],
      nextCursor: null,
      canManage: true,
    });
    expect(first.request.params.get('team')).toBeNull();
    expect(await axeViolations(dialog())).toEqual([]);
    await type(dialog().querySelector('input.resource-input'), 'car');
    await new Promise((resolve) => setTimeout(resolve, 300));
    // The debounced search runs at the next tick.
    await settle();
    await reply((url) => url.startsWith('/api/v1/resources?') && url.includes('search=car'), {
      items: [CAROL],
      nextCursor: null,
      canManage: true,
    });
    const panel = Array.from(root().querySelectorAll('.mat-mdc-autocomplete-panel')).at(-1);
    await click(Array.from(panel?.querySelectorAll('mat-option') ?? []).find((o) => o.textContent?.includes('Carol')) ?? null);
    expect((dialog().querySelector('button[type="submit"]') as HTMLButtonElement).disabled).toBe(false);
    await click(dialog().querySelector('button[type="submit"]'));
    const added = await reply((url, method) => url === `/api/v1/teams/${PLATFORM.id}/members/${CAROL.id}` && method === 'PUT', null, 204);
    expect(added.request.method).toBe('PUT');
    await closed();
    await reply((url) => url === `/api/v1/teams/${PLATFORM.id}`, { ...PLATFORM, memberCount: 3, version: 4 });
    await reply(isMembers, { items: [ALICE, BOB, CAROL], nextCursor: null, canManage: true });
    expect(text(root().querySelector('[role="status"]'))).toBe('Carol Leroy was added to the team.');
    expect(rows().length).toBe(3);

    await click((fixture.nativeElement as HTMLElement).querySelector(`#team-remove-${BOB.id}`));
    const removed = await reply((url, method) => url === `/api/v1/teams/${PLATFORM.id}/members/${BOB.id}` && method === 'DELETE', null, 204);
    expect(removed.request.method).toBe('DELETE');
    await reply((url) => url === `/api/v1/teams/${PLATFORM.id}`, { ...PLATFORM, memberCount: 2, version: 5 });
    await reply(isMembers, { items: [ALICE, CAROL], nextCursor: null, canManage: true });
    expect(text(root().querySelector('[role="status"]'))).toBe('Bob Durand was removed from the team.');
    expect(rows().length).toBe(2);
  });

  it('offers the resources already in the team as unavailable', async () => {
    const { root, click, reply, dialog, fixture, closed } = await open('tenant_admin');
    await click((fixture.nativeElement as HTMLElement).querySelector('#team-add'));
    await reply((url) => url.startsWith('/api/v1/resources?') && url.includes('active=true'), {
      items: [ALICE, CAROL],
      nextCursor: null,
      canManage: true,
    });
    (dialog().querySelector('input.resource-input') as HTMLInputElement).dispatchEvent(new Event('focusin'));
    await new Promise((resolve) => setTimeout(resolve, 50));
    TestBed.tick();
    const options = Array.from(root().querySelectorAll('mat-option'));
    const alice = options.find((o) => o.textContent?.includes('Alice'));
    expect(alice?.getAttribute('aria-disabled')).toBe('true');
    expect(alice?.textContent).toContain('already in the team');
    expect(options.find((o) => o.textContent?.includes('Carol'))?.getAttribute('aria-disabled')).toBe('false');
    // Closed before the page goes: the focus returns to a live button.
    TestBed.inject(MatDialog).closeAll();
    await closed();
  });

  it('is read-only for a member, and says when the team is empty or gone', async () => {
    const { root, rows } = await open('member', []);
    expect(rows().length).toBe(0);
    expect(root().querySelector('#team-add')).toBeNull();
    expect(root().querySelector('ui-empty-state')?.textContent).toContain('No resource in this team');
    TestBed.inject(HttpTestingController).verify();
    TestBed.resetTestingModule();

    const kit = setup();
    await signIn(kit, 'member');
    const fixture = TestBed.createComponent(TeamPage);
    fixture.componentRef.setInput('id', PLATFORM.id);
    await kit.settle();
    await kit.reply((url) => url === `/api/v1/teams/${PLATFORM.id}`, problem(404, 'not_found'), 404);
    await kit.reply(isMembers, { items: [], nextCursor: null, canManage: false });
    expect(kit.root().querySelector('ui-error-state')).not.toBeNull();
  });
});
