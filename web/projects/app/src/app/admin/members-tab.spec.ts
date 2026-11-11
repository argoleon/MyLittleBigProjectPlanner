/**
 * Tests of the "Members" tab of the administration: the filters (unit), and
 * the tab with the API mocked at the HTTP level and the real translations
 * (integration): table, filters, search, invite, change access, remove.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { AdminPage } from './admin-page';
import { MemberRow, MembersTab, memberQuery, rowState } from './members-tab';

const LUCAS = {
  accountId: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a01',
  email: 'lucas.bernard@acme.example',
  displayName: 'Lucas Bernard',
  accountType: 'internal',
  tenantRole: 'tenant_admin',
  localAccount: false,
  status: 'active',
  mfaEnabled: true,
  groups: [
    { id: 'g1', name: 'Managers' },
    { id: 'g2', name: 'Platform' },
  ],
  version: 3,
};

const PAUL = {
  ...LUCAS,
  accountId: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a02',
  email: 'p.durand@client.example',
  displayName: 'Paul Durand',
  accountType: 'external',
  tenantRole: 'member',
  mfaEnabled: false,
  groups: [],
  version: 1,
};

const INVITATION = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a10',
  email: 's.leclerc@fournisseur.example',
  accountType: 'external',
  tenantRole: 'member',
  status: 'pending',
  expiresAt: '2026-11-12T09:00:00Z',
  invitedBy: 'Lucas Bernard',
  createdAt: '2026-11-05T09:00:00Z',
  version: 1,
};

const page = (items: object[], nextCursor: string | null = null) => ({ items, nextCursor });

function problem(status: number, code: string, errors: object[] = []) {
  return { type: `urn:mlbpp:problem:${code}`, title: code, status, code, errors };
}

describe('memberQuery', () => {
  it('asks for both types, one, or none', () => {
    expect(memberQuery(' Lucas ', ['internal', 'external'])).toEqual({
      search: 'Lucas',
      accountType: undefined,
      pendingOnly: false,
    });
    expect(memberQuery('', ['external'])).toEqual({ search: undefined, accountType: 'external', pendingOnly: false });
    expect(memberQuery('', ['pending'])).toEqual({ search: undefined, accountType: null, pendingOnly: true });
    expect(memberQuery('', ['internal', 'pending']).pendingOnly).toBe(true);
  });

  it('labels each row with its status', () => {
    const member = (status: string) => ({ kind: 'member', id: 'x', member: { ...LUCAS, status } }) as MemberRow;
    const invitation = (status: string) =>
      ({ kind: 'invitation', id: 'y', invitation: { ...INVITATION, status } }) as MemberRow;
    expect(rowState(member('active'))).toBe('active');
    expect(rowState(member('suspended'))).toBe('suspended');
    expect(rowState(invitation('pending'))).toBe('invited');
    expect(rowState(invitation('expired'))).toBe('expired');
  });
});

async function open({
  members = [LUCAS, PAUL] as object[],
  invitations = [INVITATION] as object[],
  nextCursor = null as string | null,
} = {}) {
  localStorage.clear();
  TestBed.configureTestingModule({
    providers: [
      provideRouter([{ path: 'admin', component: AdminPage, children: [{ path: 'members', component: MembersTab }] }]),
      provideApi(),
      provideHttpClientTesting(),
      provideI18nTesting(),
      provideMaterialDefaults(),
    ],
  });
  const harness = await RouterTestingHarness.create();
  await harness.navigateByUrl('/admin/members');
  const http = TestBed.inject(HttpTestingController);
  const settle = async () => {
    for (let i = 0; i < 3; i++) {
      await new Promise((resolve) => setTimeout(resolve));
      TestBed.tick();
    }
  };
  const reply = async (
    match: (url: string) => boolean,
    body: object | null,
    status = 200,
  ): Promise<TestRequest> => {
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
  const isMembers = (url: string) => url.startsWith('/api/v1/tenant/members');
  const isInvitations = (url: string) => url.startsWith('/api/v1/tenant/invitations');
  await reply((url) => url === '/api/v1/tenant', {
    id: 't',
    slug: 'acme',
    name: 'Acme Industrie',
    defaultLocale: 'fr',
    timeZone: 'Europe/Paris',
  });
  await reply(isMembers, page(members, nextCursor));
  await reply(isInvitations, page(invitations));
  const root = () => document.body;
  const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
  const click = async (element: Element | null) => {
    (element as HTMLElement).click();
    await settle();
  };
  const rows = () => Array.from(root().querySelectorAll('tr[mat-row]'));
  return { http, reply, settle, root, text, click, rows, isMembers, isInvitations };
}

describe('MembersTab', () => {
  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify();
  });

  it('lists the invitations then the members, with type, role, groups, MFA and status', async () => {
    const { root, text, rows } = await open();
    const cells = rows().map((row) => Array.from(row.querySelectorAll('td')).map((cell) => text(cell)));
    expect(cells).toEqual([
      [
        's.leclerc@fournisseur.exampleInvitation sent on Nov 5, 2026',
        'Account type: External',
        'External · portal only',
        '',
        '–Two-step verification off',
        'Status: Invitation sent',
        'more_vert',
      ],
      [
        'Lucas Bernardlucas.bernard@acme.example',
        'Account type: Internal',
        'Organization admin',
        'Managers, Platform',
        'verified_user',
        'Status: Active',
        'more_vert',
      ],
      [
        'Paul Durandp.durand@client.example',
        'Account type: External',
        'External · portal only',
        '',
        '–Two-step verification off',
        'Status: Active',
        'more_vert',
      ],
    ]);
    expect(root().querySelector('.mfa-on')?.getAttribute('aria-label')).toBe('Two-step verification on');
    expect(root().querySelectorAll('th[scope="col"]').length).toBe(7);
    // Every action is a real button with a name, reachable with the keyboard.
    const actions = Array.from(root().querySelectorAll<HTMLButtonElement>('td button'));
    expect(actions.map((b) => b.getAttribute('aria-label'))).toEqual([
      'Actions for s.leclerc@fournisseur.example',
      'Actions for Lucas Bernard',
      'Actions for Paul Durand',
    ]);
    expect(actions.every((b) => b.tabIndex >= 0)).toBe(true);
    expect(await axeViolations(root().querySelector('app-admin-page')!)).toEqual([]);
  });

  it('filters by type and pending invitations, and searches', async () => {
    const { root, reply, click, settle, isMembers, isInvitations, rows } = await open();
    // Internal only.
    await click(root().querySelector('mat-chip-option:nth-of-type(2) button, mat-chip-option:nth-of-type(2) .mdc-evolution-chip__action'));
    const members = await reply(isMembers, page([LUCAS]));
    expect(members.request.params.get('accountType')).toBe('internal');
    const invitations = await reply(isInvitations, page([]));
    expect(invitations.request.params.get('accountType')).toBe('internal');
    expect(rows().length).toBe(1);

    // Pending invitations only: no member request.
    await click(root().querySelector('mat-chip-option:nth-of-type(3) .mdc-evolution-chip__action'));
    await reply(isInvitations, page([INVITATION]));
    expect(rows().length).toBe(1);
    expect(rows()[0].textContent).toContain('s.leclerc@fournisseur.example');

    // Back to members, searched.
    await click(root().querySelector('mat-chip-option:nth-of-type(3) .mdc-evolution-chip__action'));
    await reply(isMembers, page([LUCAS]));
    await reply(isInvitations, page([]));
    const search = root().querySelector('input[type="search"]') as HTMLInputElement;
    search.value = 'lucas';
    search.dispatchEvent(new Event('input'));
    await new Promise((resolve) => setTimeout(resolve, 350));
    await settle();
    const searched = await reply(isMembers, page([LUCAS]));
    expect(searched.request.params.get('search')).toBe('lucas');
    await reply(isInvitations, page([]));
  });

  it('shows an empty state', async () => {
    const { root, text } = await open({ members: [], invitations: [] });
    expect(text(root().querySelector('ui-empty-state [role="heading"]'))).toBe('No member matches');
  });

  it('loads the next page of members', async () => {
    const { root, reply, click, rows, isMembers } = await open({ members: [LUCAS], invitations: [], nextCursor: 'c1' });
    expect(rows().length).toBe(1);
    await click(root().querySelector('.more'));
    const next = await reply(isMembers, page([PAUL]));
    expect(next.request.params.get('cursor')).toBe('c1');
    expect(rows().map((row) => row.querySelector('.name')?.textContent)).toEqual(['Lucas Bernard', 'Paul Durand']);
    expect(root().querySelector('.more')).toBeNull();
  });

  it('invites a member through the dialog, an external one as a plain member', async () => {
    const { root, reply, click, settle, isInvitations, text } = await open();
    await click(root().querySelector('.invite'));
    const dialog = () => root().querySelector('mat-dialog-container')!;
    expect(text(dialog().querySelector('h2'))).toBe('Invite a member');
    await click(dialog().querySelector('button[type="submit"]'));
    expect(text(dialog().querySelector('mat-error'))).toBe('This field is required.');

    const email = dialog().querySelector('input[formcontrolname="email"]') as HTMLInputElement;
    email.value = 'p.durand@client.example';
    email.dispatchEvent(new Event('input'));
    await click(dialog().querySelectorAll('mat-button-toggle button')[1]);
    expect(dialog().querySelector('mat-select')?.getAttribute('aria-disabled')).toBe('true');
    expect(text(dialog().querySelector('mat-hint'))).toBe('External accounts are plain members.');
    await settle();
    await click(dialog().querySelector('button[type="submit"]'));
    const sent = await reply((url) => url === '/api/v1/tenant/invitations', { ...INVITATION, email: 'p.durand@client.example' }, 201);
    expect(sent.request.method).toBe('POST');
    expect(sent.request.body).toEqual({ email: 'p.durand@client.example', accountType: 'external', tenantRole: 'member' });
    expect(sent.request.headers.get('Idempotency-Key')).toMatch(/^[0-9a-f-]{36}$/);
    await reply(isInvitations, page([INVITATION]));
    expect(text(root().querySelector('[role="status"]'))).toBe('Invitation sent to p.durand@client.example.');
  });

  it('shows why an invitation is refused', async () => {
    const { root, reply, click, text } = await open();
    await click(root().querySelector('.invite'));
    const dialog = () => root().querySelector('mat-dialog-container')!;
    const email = dialog().querySelector('input[formcontrolname="email"]') as HTMLInputElement;
    email.value = 'lucas.bernard@acme.example';
    email.dispatchEvent(new Event('input'));
    await click(dialog().querySelector('button[type="submit"]'));
    await reply((url) => url === '/api/v1/tenant/invitations', problem(409, 'invitation.already_member'), 409);
    expect(text(dialog().querySelector('[role="alert"]'))).toBe('This email already belongs to a member of the organization.');
  });

  it('changes a member’s role with the version it read', async () => {
    const { root, reply, click, text, isMembers, isInvitations } = await open();
    await click(root().querySelector('button[aria-label="Actions for Lucas Bernard"]'));
    await click(root().querySelector('.mat-mdc-menu-panel button'));
    const dialog = () => root().querySelector('mat-dialog-container')!;
    expect(text(dialog().querySelector('h2'))).toBe('Access of Lucas Bernard');
    await click(dialog().querySelector('mat-select .mat-mdc-select-trigger'));
    await click(Array.from(root().querySelectorAll('mat-option')).find((o) => o.textContent?.includes('Configuration admin'))!);
    await click(dialog().querySelector('button[type="submit"]'));
    const put = await reply(
      (url) => url === `/api/v1/tenant/members/${LUCAS.accountId}`,
      { ...LUCAS, tenantRole: 'configuration_admin', version: 4 },
    );
    expect(put.request.method).toBe('PUT');
    expect(put.request.headers.get('If-Match')).toBe('"3"');
    expect(put.request.body).toEqual({ accountType: 'internal', tenantRole: 'configuration_admin', localAccount: false });
    await reply(isMembers, page([LUCAS, PAUL]));
    await reply(isInvitations, page([INVITATION]));
    expect(text(root().querySelector('[role="status"]'))).toBe('Access of Lucas Bernard changed.');
  });

  it('gives the focus back to the actions button when a dialog of its menu closes', async () => {
    // The menu gives the focus back to its button only once closed, after
    // the dialog has opened: the dialog must not rely on the focused element.
    const { root, click } = await open();
    const open$ = vi.spyOn(TestBed.inject(MatDialog), 'open');
    const cases: [string, number][] = [
      ['Actions for Lucas Bernard', 0],
      ['Actions for Lucas Bernard', 1],
      [`Actions for ${INVITATION.email}`, 1],
    ];
    for (const [label, item] of cases) {
      const trigger = root().querySelector(`button[aria-label="${label}"]`);
      await click(trigger);
      // The last menu opened: a closed one may still be leaving.
      const menu = Array.from(root().querySelectorAll('.mat-mdc-menu-panel')).at(-1)!;
      await click(menu.querySelectorAll('button')[item]);
      const restoreFocus = open$.mock.lastCall?.[1]?.restoreFocus;
      expect(typeof restoreFocus).toBe('string');
      expect(root().querySelector(restoreFocus as string)).toBe(trigger);
      TestBed.inject(MatDialog).closeAll();
    }
  });

  it('removes a member after confirmation, and reports the last admin', async () => {
    const { root, reply, click, text } = await open();
    await click(root().querySelector('button[aria-label="Actions for Lucas Bernard"]'));
    await click(root().querySelectorAll('.mat-mdc-menu-panel button')[1]);
    const dialog = () => root().querySelector('mat-dialog-container')!;
    expect(text(dialog().querySelector('h2'))).toBe('Remove Lucas Bernard?');
    await click(dialog().querySelectorAll('button')[1]);
    const removed = await reply(
      (url) => url === `/api/v1/tenant/members/${LUCAS.accountId}`,
      problem(409, 'member.last_admin'),
      409,
    );
    expect(removed.request.method).toBe('DELETE');
    expect(removed.request.headers.get('If-Match')).toBe('"3"');
    expect(text(root().querySelector('[role="alert"]'))).toBe(
      'The organization needs at least one active organization admin.',
    );
  });

  it('sends an invitation again, and revokes it', async () => {
    const { root, reply, click, text, isInvitations } = await open();
    await click(root().querySelector('button[aria-label="Actions for s.leclerc@fournisseur.example"]'));
    await click(root().querySelector('.mat-mdc-menu-panel button'));
    const resent = await reply((url) => url === `/api/v1/tenant/invitations/${INVITATION.id}/resend`, INVITATION);
    expect(resent.request.method).toBe('POST');
    await reply(isInvitations, page([INVITATION]));
    expect(text(root().querySelector('[role="status"]'))).toBe('Invitation sent again to s.leclerc@fournisseur.example.');

    await click(root().querySelector('button[aria-label="Actions for s.leclerc@fournisseur.example"]'));
    await click(root().querySelectorAll('.mat-mdc-menu-panel button')[1]);
    await click(root().querySelector('mat-dialog-container')!.querySelectorAll('button')[1]);
    const revoked = await reply((url) => url === `/api/v1/tenant/invitations/${INVITATION.id}`, null, 204);
    expect(revoked.request.method).toBe('DELETE');
    expect(revoked.request.headers.get('If-Match')).toBe('"1"');
    await reply(isInvitations, page([]));
  });

  it('shows the refusal of a configuration admin', async () => {
    localStorage.clear();
    const { root, text } = await openRefused();
    expect(text(root().querySelector('ui-error-state'))).toContain('You are not allowed to do this.');
  });
});

/** The tab when the API refuses (not a tenant admin). */
async function openRefused() {
  TestBed.configureTestingModule({
    providers: [
      provideRouter([{ path: 'admin', component: AdminPage, children: [{ path: 'members', component: MembersTab }] }]),
      provideApi(),
      provideHttpClientTesting(),
      provideI18nTesting(),
      provideMaterialDefaults(),
    ],
  });
  const harness = await RouterTestingHarness.create();
  await harness.navigateByUrl('/admin/members');
  const http = TestBed.inject(HttpTestingController);
  const settle = async () => {
    for (let i = 0; i < 3; i++) {
      await new Promise((resolve) => setTimeout(resolve));
      TestBed.tick();
    }
  };
  await settle();
  for (const request of http.match(() => true)) {
    if (request.request.url === '/api/v1/tenant') {
      request.flush({ id: 't', slug: 'acme', name: 'Acme', defaultLocale: 'fr', timeZone: 'Europe/Paris' });
    } else {
      request.flush(problem(403, 'forbidden'), { status: 403, statusText: 'Forbidden' });
    }
  }
  await settle();
  const root = () => document.body;
  const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
  return { root, text };
}
