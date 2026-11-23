/**
 * Tests of the resources (L07-US01): the query of the list and the body of
 * the dialog (unit), and the list, the dialog and the resource page with
 * the API mocked at the HTTP level and the real translations
 * (integration): table, filters, pages, create and edit, errors, rights,
 * accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter } from '@angular/router';
import { AuthService, Resource, Session, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { managesResources, ResourcePage } from './resource-page';
import { resourceBody } from './resource-dialog';
import { ResourcesPage } from './resources-page';
import { resourceQuery } from './resources-store';

const CAMILLE: Resource = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b01',
  kind: 'person',
  displayName: 'Camille Martin',
  jobTitle: 'Dev back',
  accountId: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a01',
  formerMember: false,
  active: true,
  version: 2,
};
const BOB: Resource = {
  ...CAMILLE,
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b02',
  displayName: 'Bob Subcontractor',
  jobTitle: null,
  accountId: null,
  version: 1,
};
const RUST: Resource = {
  ...BOB,
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b03',
  kind: 'generic',
  displayName: 'Rust developer',
};

const page = (items: Resource[], nextCursor: string | null = null, canManage = true) => ({ items, nextCursor, canManage });

function problem(status: number, code: string, errors: object[] = []) {
  return { type: `urn:mlbpp:problem:${code}`, title: code, status, code, errors };
}

describe('resourceQuery', () => {
  it('asks for one kind or both, active ones or all, or nothing', () => {
    expect(resourceQuery(' rust ', ['person', 'generic'], ['active'])).toEqual({
      search: 'rust',
      kind: undefined,
      active: true,
      none: false,
    });
    expect(resourceQuery('', ['generic'], ['active', 'inactive'])).toEqual({
      search: undefined,
      kind: 'generic',
      active: undefined,
      none: false,
    });
    expect(resourceQuery('', ['person'], ['inactive']).active).toBe(false);
    expect(resourceQuery('', [], ['active']).none).toBe(true);
    expect(resourceQuery('', ['person'], []).none).toBe(true);
  });
});

describe('resourceBody', () => {
  it('sends the account of a linked person, the name otherwise, trimmed', () => {
    const camille = { accountId: 'a1', displayName: 'Camille Martin' };
    expect(resourceBody('person', camille, 'ignored', ' Dev back ', true)).toEqual({
      kind: 'person',
      accountId: 'a1',
      displayName: null,
      jobTitle: 'Dev back',
      active: true,
    });
    expect(resourceBody('person', null, ' Bob ', '', false)).toEqual({
      kind: 'person',
      accountId: null,
      displayName: 'Bob',
      jobTitle: null,
      active: false,
    });
    // A generic resource is never linked.
    expect(resourceBody('generic', camille, 'Rust developer', '', true).accountId).toBeNull();
  });

  it('lets tenant admins and resource managers manage', () => {
    expect(managesResources('tenant_admin')).toBe(true);
    expect(managesResources('resource_manager')).toBe(true);
    expect(managesResources('configuration_admin')).toBe(false);
    expect(managesResources('member')).toBe(false);
    expect(managesResources(undefined)).toBe(false);
  });
});

function setup() {
  localStorage.clear();
  TestBed.configureTestingModule({
    providers: [provideRouter([]), provideApi(), provideHttpClientTesting(), provideI18nTesting(), provideMaterialDefaults()],
  });
  const http = TestBed.inject(HttpTestingController);
  const settle = async () => {
    for (let i = 0; i < 3; i++) {
      await new Promise((resolve) => setTimeout(resolve, 10));
      TestBed.tick();
    }
  };
  const reply = async (match: (url: string) => boolean, body: object | null, status = 200): Promise<TestRequest> => {
    let request: TestRequest | undefined;
    for (let i = 0; i < 50 && !request; i++) {
      [request] = http.match((r) => match(r.urlWithParams));
      if (!request) await new Promise((resolve) => setTimeout(resolve, 20));
    }
    if (!request) throw new Error('no matching request');
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
  return { http, settle, reply, root, text, click, type, dialog };
}

const isList = (url: string) => url.startsWith('/api/v1/resources?');
const isLinkable = (url: string) => url.startsWith('/api/v1/resources/linkable-accounts');

describe('ResourcesPage', () => {
  async function open(first = page([BOB, CAMILLE, RUST])) {
    const kit = setup();
    const fixture = TestBed.createComponent(ResourcesPage);
    await kit.settle();
    const request = await kit.reply(isList, first);
    const rows = () => Array.from(kit.root().querySelectorAll('tr[mat-row]'));
    return { ...kit, fixture, request, rows };
  }

  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify();
  });

  it('lists the active resources by name, each opening its page', async () => {
    const { root, text, rows, request, fixture } = await open();
    expect(request.request.params.get('active')).toBe('true');
    expect(request.request.params.get('kind')).toBeNull();
    expect(
      rows().map((row) => [
        text(row.querySelector('.name')),
        text(row.querySelector('.secondary')),
        ...Array.from(row.querySelectorAll('td')).slice(1).map((cell) => text(cell)),
      ]),
    ).toEqual([
      ['Bob Subcontractor', undefined, 'Resource type: Person', 'Status: Active', 'edit'],
      ['Camille Martin', 'Dev back', 'Resource type: Person · linked account', 'Status: Active', 'edit'],
      ['Rust developer', undefined, 'Resource type: Generic resource', 'Status: Active', 'edit'],
    ]);
    expect(root().querySelector('a.resource')?.getAttribute('href')).toBe(`/workload/resources/${BOB.id}`);
    expect(Array.from(root().querySelectorAll('td button')).map((b) => b.getAttribute('aria-label'))).toEqual([
      'Edit Bob Subcontractor',
      'Edit Camille Martin',
      'Edit Rust developer',
    ]);
    expect(text(root().querySelector('button.new'))).toContain('New resource');
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
  });

  it('shows the list read-only to a member who does not manage resources', async () => {
    const { root, rows } = await open(page([BOB], null, false));
    expect(rows().length).toBe(1);
    expect(root().querySelector('button.new')).toBeNull();
    expect(root().querySelector('td button')).toBeNull();
  });

  it('filters by kind and status, searches, and loads the next pages', async () => {
    const { root, reply, click, type, rows } = await open(page([BOB, CAMILLE], 'c2'));
    // Generic resources only.
    await click(root().querySelector('.kinds mat-chip-option:nth-of-type(1) .mdc-evolution-chip__action'));
    const generic = await reply(isList, page([RUST]));
    expect(generic.request.params.get('kind')).toBe('generic');
    expect(rows().length).toBe(1);
    // Inactive ones too.
    await click(root().querySelector('.states mat-chip-option:nth-of-type(2) .mdc-evolution-chip__action'));
    const all = await reply(isList, page([RUST], 'next'));
    expect(all.request.params.get('active')).toBeNull();
    // The next page follows the cursor.
    await click(root().querySelector('button.more'));
    const more = await reply(isList, page([{ ...RUST, id: 'r2', displayName: 'Rust lead' }]));
    expect(more.request.params.get('cursor')).toBe('next');
    expect(rows().length).toBe(2);
    expect(root().querySelector('button.more')).toBeNull();
    // A search.
    await type(root().querySelector('input[type="search"]'), 'lead');
    await new Promise((resolve) => setTimeout(resolve, 350));
    const searched = await reply(isList, page([]));
    expect(searched.request.params.get('search')).toBe('lead');
    expect(root().querySelector('ui-empty-state')?.textContent).toContain('No resource matches');
  });

  it('says when there is no resource yet, and when the list cannot be read', async () => {
    const { root } = await open(page([]));
    expect(root().querySelector('ui-empty-state')?.textContent).toContain('No resource yet');
    TestBed.resetTestingModule();
    const kit = setup();
    TestBed.createComponent(ResourcesPage);
    await kit.settle();
    await kit.reply(isList, problem(500, 'internal'), 500);
    expect(kit.root().querySelector('ui-error-state')).not.toBeNull();
  });

  it('creates a person linked to a member found by name', async () => {
    const { root, reply, click, type, text, http, dialog } = await open(page([BOB]));
    await click(root().querySelector('button.new'));
    await reply(isLinkable, [{ accountId: 'a1', displayName: 'Camille Martin', email: 'camille@acme.example' }]);
    expect(await axeViolations(dialog())).toEqual([]);

    const account = root().querySelector('input.account-input');
    await type(account, 'cam');
    await new Promise((resolve) => setTimeout(resolve, 300));
    const searched = await reply(isLinkable, [{ accountId: 'a1', displayName: 'Camille Martin', email: 'camille@acme.example' }]);
    expect(searched.request.params.get('search')).toBe('cam');
    await click(Array.from(root().querySelectorAll('mat-option')).find((o) => o.textContent?.includes('Camille Martin')) ?? null);
    // Linked: the name is the member's, no name field.
    expect(dialog().querySelectorAll('input[formcontrolname="displayName"]').length).toBe(0);
    await type(root().querySelector('input[formcontrolname="jobTitle"]'), 'Dev back');
    await click(dialog().querySelector('button[type="submit"]'));
    const created = await reply((url) => url === '/api/v1/resources', { ...CAMILLE, version: 1 }, 201);
    expect(created.request.method).toBe('POST');
    expect(created.request.headers.get('Idempotency-Key')).toBeTruthy();
    expect(created.request.body).toEqual({
      kind: 'person',
      accountId: 'a1',
      displayName: null,
      jobTitle: 'Dev back',
      active: true,
    });
    await reply(isList, page([BOB, CAMILLE]));
    expect(text(root().querySelector('[role="status"]'))).toBe('Camille Martin was created.');
    http.match(isLinkable as never);
  });

  it('creates a generic resource, and shows why one is refused', async () => {
    const { root, reply, click, type, text, dialog } = await open(page([]));
    await click(root().querySelector('button.new'));
    await reply(isLinkable, []);
    await click(root().querySelector('mat-button-toggle[value="generic"] button'));
    expect(root().querySelector('input.account-input')).toBeNull();
    // The name is required.
    await click(dialog().querySelector('button[type="submit"]'));
    expect(text(dialog().querySelector('mat-error'))).toBe('This field is required.');
    await type(root().querySelector('input[formcontrolname="displayName"]'), 'Rust developer');
    await click(dialog().querySelector('button[type="submit"]'));
    const refused = await reply(
      (url) => url === '/api/v1/resources',
      problem(400, 'validation_failed', [{ field: 'displayName', code: 'too_long' }]),
      400,
    );
    expect(refused.request.body).toMatchObject({ kind: 'generic', displayName: 'Rust developer', accountId: null });
    expect(text(dialog().querySelector('mat-error'))).toBe('The text is too long.');
    expect(dialog().querySelector('[role="alert"]')).toBeNull();
  });

  it('edits a resource with its version, and tells when the member is linked already', async () => {
    const { root, reply, click, text, http, dialog } = await open(page([BOB]));
    await click(root().querySelector('td button'));
    await reply(isLinkable, [{ accountId: 'a1', displayName: 'Camille Martin', email: 'camille@acme.example' }]);
    expect(text(dialog().querySelector('h2'))).toBe('Edit Bob Subcontractor');
    expect((root().querySelector('input[formcontrolname="displayName"]') as HTMLInputElement).value).toBe('Bob Subcontractor');
    // Deactivated.
    await click(dialog().querySelector('mat-slide-toggle button'));
    await click(dialog().querySelector('button[type="submit"]'));
    const saved = await reply((url) => url === `/api/v1/resources/${BOB.id}`, { ...BOB, active: false, version: 2 });
    expect(saved.request.method).toBe('PUT');
    expect(saved.request.headers.get('If-Match')).toBe('"1"');
    expect(saved.request.body).toEqual({ kind: 'person', accountId: null, displayName: 'Bob Subcontractor', jobTitle: null, active: false });
    await reply(isList, page([]));
    expect(text(root().querySelector('[role="status"]'))).toBe('Bob Subcontractor was saved.');

    // A member linked to another resource meanwhile.
    await click(root().querySelector('button.new'));
    await reply(isLinkable, [{ accountId: 'a1', displayName: 'Camille Martin', email: 'camille@acme.example' }]);
    const account = root().querySelector('input.account-input') as HTMLInputElement;
    account.dispatchEvent(new Event('focusin'));
    await new Promise((resolve) => setTimeout(resolve, 50));
    TestBed.tick();
    await click(Array.from(root().querySelectorAll('mat-option')).find((o) => o.textContent?.includes('Camille')) ?? null);
    await click(dialog().querySelector('button[type="submit"]'));
    await reply(
      (url) => url === '/api/v1/resources',
      problem(409, 'resource.account_taken', [{ field: 'accountId', code: 'taken' }]),
      409,
    );
    expect(text(dialog().querySelector('[role="alert"]'))).toBe(
      'This member is already linked to another resource.',
    );
    http.match(isLinkable as never);
  });
});

describe('ResourcePage', () => {
  async function open(role: Session['account']['tenantRole'], answer: { body: object; status?: number }) {
    const kit = setup();
    // The session the guard loads before the page.
    const loaded = TestBed.inject(AuthService).load();
    const session: Session = {
      account: { id: 'me', email: 'me@acme.example', displayName: 'Me', tenantRole: role },
      csrfToken: 'csrf',
      expiresAt: '2026-12-01T00:00:00Z',
      stage: 'complete',
    };
    await kit.reply((url) => url === '/api/v1/auth/session', session);
    await loaded;
    const fixture = TestBed.createComponent(ResourcePage);
    fixture.componentRef.setInput('id', CAMILLE.id);
    await kit.settle();
    await kit.reply((url) => url === `/api/v1/resources/${CAMILLE.id}`, answer.body, answer.status ?? 200);
    return { ...kit, fixture };
  }

  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
  });

  it('shows the resource, editable by a resource manager', async () => {
    const { root, text, click, reply, fixture, dialog } = await open('resource_manager', { body: CAMILLE });
    expect(text(root().querySelector('h1'))).toBe('Camille Martin');
    const chips = () => Array.from(root().querySelectorAll('.chips > *')).map((chip) => text(chip));
    expect(chips()).toEqual(['Resource type: Person · linked account', 'Dev back', 'Status: Active']);
    expect(Array.from(root().querySelectorAll('.details dd')).map((dd) => text(dd))).toEqual([
      'Person · linked account',
      'Linked to the member’s account',
      'Dev back',
      'Active',
    ]);
    expect(root().querySelector('.breadcrumb a')?.getAttribute('href')).toBe('/workload/resources');
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);

    await click(root().querySelector('button.edit'));
    await reply(isLinkable, []);
    await click(dialog().querySelector('button[type="submit"]'));
    const saved = await reply((url) => url === `/api/v1/resources/${CAMILLE.id}`, { ...CAMILLE, jobTitle: 'Lead dev', version: 3 });
    expect(saved.request.method).toBe('PUT');
    expect(saved.request.headers.get('If-Match')).toBe('"2"');
    await new Promise((resolve) => setTimeout(resolve, 200));
    fixture.detectChanges();
    expect(chips()).toContain('Lead dev');
  });

  it('is read-only for a member, and tells a former member apart', async () => {
    const { root, text } = await open('member', { body: { ...BOB, formerMember: true, active: false } });
    expect(root().querySelector('button.edit')).toBeNull();
    expect(Array.from(root().querySelectorAll('.chips > *')).map((chip) => text(chip))).toEqual([
      'Resource type: Former member',
      'Status: Inactive',
    ]);
    expect(text(root().querySelectorAll('.details dd')[1])).toBe('Former member: unlinked when they left the organization');
  });

  it('tells when the resource cannot be found', async () => {
    const { root } = await open('member', { body: problem(404, 'not_found'), status: 404 });
    expect(root().querySelector('ui-error-state')).not.toBeNull();
  });
});
