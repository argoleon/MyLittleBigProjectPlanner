/**
 * Integration tests of the "Groups" tab of the administration, with the API
 * mocked at the HTTP level and the real translations.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { AdminPage } from './admin-page';
import { GroupsTab } from './groups-tab';

const PLATFORM = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a20',
  name: 'Platform',
  description: 'Platform team',
  memberCount: 2,
  version: 4,
};

const LUCAS = {
  accountId: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a01',
  email: 'lucas.bernard@acme.example',
  displayName: 'Lucas Bernard',
};

const INES = {
  accountId: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a03',
  email: 'ines.moreau@acme.example',
  displayName: 'Inès Moreau',
  accountType: 'internal',
  tenantRole: 'member',
  localAccount: false,
  status: 'active',
  mfaEnabled: false,
  groups: [],
  version: 1,
};

function problem(status: number, code: string, errors: object[] = []) {
  return { type: `urn:mlbpp:problem:${code}`, title: code, status, code, errors };
}

async function open(groups: object[] = [PLATFORM]) {
  localStorage.clear();
  TestBed.configureTestingModule({
    providers: [
      provideRouter([{ path: 'admin', component: AdminPage, children: [{ path: 'groups', component: GroupsTab }] }]),
      provideApi(),
      provideHttpClientTesting(),
      provideI18nTesting(),
      provideMaterialDefaults(),
    ],
  });
  const harness = await RouterTestingHarness.create();
  await harness.navigateByUrl('/admin/groups');
  const http = TestBed.inject(HttpTestingController);
  const settle = async () => {
    for (let i = 0; i < 3; i++) {
      await new Promise((resolve) => setTimeout(resolve));
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
  const isList = (url: string) => url.startsWith('/api/v1/tenant/groups?');
  await reply((url) => url === '/api/v1/tenant', {
    id: 't',
    slug: 'acme',
    name: 'Acme Industrie',
    defaultLocale: 'fr',
    timeZone: 'Europe/Paris',
  });
  await reply(isList, { items: groups, nextCursor: null });
  const root = () => document.body;
  const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
  const click = async (element: Element | null) => {
    (element as HTMLElement).click();
    await settle();
  };
  const dialog = () => root().querySelector('mat-dialog-container:last-of-type')!;
  return { reply, settle, root, text, click, dialog, isList };
}

describe('GroupsTab', () => {
  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify();
  });

  it('lists the groups with their number of members', async () => {
    const { root, text } = await open();
    const row = root().querySelector('tr[mat-row]');
    expect(text(row?.querySelector('td'))).toBe('PlatformPlatform team');
    expect(text(row?.querySelectorAll('td')[1])).toBe('2');
    expect(root().querySelector('button[aria-label="Edit Platform"]')).not.toBeNull();
    expect(root().querySelector('button[aria-label="Delete Platform"]')).not.toBeNull();
    expect(await axeViolations(root().querySelector('app-admin-page')!)).toEqual([]);
  });

  it('shows an empty state, then creates a group', async () => {
    const { root, text, click, reply, dialog, isList } = await open([]);
    expect(text(root().querySelector('ui-empty-state [role="heading"]'))).toBe('No group yet');
    await click(root().querySelector('.invite'));
    expect(text(dialog().querySelector('h2'))).toBe('New group');
    const name = dialog().querySelector('input[formcontrolname="name"]') as HTMLInputElement;
    name.value = ' Platform ';
    name.dispatchEvent(new Event('input'));
    await click(dialog().querySelector('button[type="submit"]'));
    // The name is taken: said on the field.
    await reply((url) => url === '/api/v1/tenant/groups', problem(409, 'group.name_taken', [{ field: 'name', code: 'taken' }]), 409);
    expect(text(dialog().querySelector('mat-error'))).toBe('This value is already used.');
    name.value = 'Platform team';
    name.dispatchEvent(new Event('input'));
    await click(dialog().querySelector('button[type="submit"]'));
    const created = await reply((url) => url === '/api/v1/tenant/groups', PLATFORM, 201);
    expect(created.request.body).toEqual({ name: 'Platform team', description: null });
    expect(created.request.headers.get('Idempotency-Key')).toMatch(/^[0-9a-f-]{36}$/);
    await reply(isList, { items: [PLATFORM], nextCursor: null });
    expect(text(root().querySelector('[role="status"]'))).toBe('Group Platform saved.');
  });

  it('adds and removes members of a group', async () => {
    const { root, text, click, reply, settle, dialog, isList } = await open();
    await click(root().querySelector('.group-actions button'));
    expect(text(dialog().querySelector('h2'))).toBe('Members of Platform');
    const detail = { ...PLATFORM, members: [LUCAS] };
    await reply((url) => url === `/api/v1/tenant/groups/${PLATFORM.id}`, detail);
    expect(text(dialog().querySelector('.member'))).toContain('Lucas Bernard');

    const search = dialog().querySelector('input[type="search"]') as HTMLInputElement;
    search.focus();
    search.value = 'ines';
    search.dispatchEvent(new Event('input'));
    await new Promise((resolve) => setTimeout(resolve, 300));
    await settle();
    const found = await reply((url) => url.startsWith('/api/v1/tenant/members?'), { items: [INES], nextCursor: null });
    expect(found.request.params.get('search')).toBe('ines');
    await click(Array.from(root().querySelectorAll('mat-option')).find((o) => o.textContent?.includes('Inès Moreau'))!);
    const added = await reply((url) => url === `/api/v1/tenant/groups/${PLATFORM.id}/members/${INES.accountId}`, null, 204);
    expect(added.request.method).toBe('PUT');
    await reply((url) => url === `/api/v1/tenant/groups/${PLATFORM.id}`, { ...detail, members: [INES, LUCAS] });

    await click(dialog().querySelector('button[aria-label="Remove Lucas Bernard from the group"]'));
    const removed = await reply((url) => url === `/api/v1/tenant/groups/${PLATFORM.id}/members/${LUCAS.accountId}`, null, 204);
    expect(removed.request.method).toBe('DELETE');
    await reply((url) => url === `/api/v1/tenant/groups/${PLATFORM.id}`, { ...detail, members: [INES] });
    expect(dialog().querySelectorAll('.member').length).toBe(1);

    TestBed.inject(MatDialog).closeAll();
    await reply(isList, { items: [PLATFORM], nextCursor: null });
  });

  it('deletes a group after confirmation, with its version', async () => {
    const { root, click, reply, dialog, isList } = await open();
    await click(root().querySelector('button[aria-label="Delete Platform"]'));
    await click(dialog().querySelectorAll('button')[1]);
    const deleted = await reply((url) => url === `/api/v1/tenant/groups/${PLATFORM.id}`, null, 204);
    expect(deleted.request.method).toBe('DELETE');
    expect(deleted.request.headers.get('If-Match')).toBe('"4"');
    await reply(isList, { items: [], nextCursor: null });
  });
});
