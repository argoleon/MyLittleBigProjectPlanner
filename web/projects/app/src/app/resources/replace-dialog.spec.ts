/**
 * Tests of replacing a generic resource with a person (L08-US08): the
 * tickets in words (unit), and the resource page's "Replace with a person"
 * button and its dialog with the API mocked at the HTTP level and the real
 * translations (integration): offered on generic resources to those who
 * manage resources, the people searched, the preview of what moves, the
 * generic resource deactivated by default, overlaps and overloads
 * blocking, forcing for those allowed, the page told and read again,
 * accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter } from '@angular/router';
import { AuthService, Resource, ResourceReplacement, Session, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { ticketsText } from './replace-dialog';
import { ResourcePage } from './resource-page';

const RUST: Resource = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b03',
  kind: 'generic',
  displayName: 'Rust developer',
  jobTitle: null,
  accountId: null,
  formerMember: false,
  active: true,
  teams: [],
  site: null,
  version: 1,
};
const NATHAN: Resource = {
  ...RUST,
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b04',
  kind: 'person',
  displayName: 'Nathan Roux',
  jobTitle: 'Back-end developer',
};
const PREVIEW = `/api/v1/resources/${RUST.id}/replacement/preview`;
const REPLACE = `/api/v1/resources/${RUST.id}/replacement`;

function told(overrides: Partial<ResourceReplacement> = {}): ResourceReplacement {
  return {
    from: '2030-03-11',
    moved: [
      { issueId: 'i1', issueKey: 'PLAT-1', merged: false, start: '2030-03-11', end: '2030-03-15' },
      { issueId: null, issueKey: null, merged: true, start: '2030-03-18', end: '2030-03-19' },
    ],
    overlaps: [],
    archived: [],
    overloads: [],
    mode: 'blocking',
    blocked: false,
    forceable: false,
    deactivated: false,
    ...overrides,
  };
}

const OVER = { date: '2030-03-11', plannedMinutes: 540, availableMinutes: 420, limitMinutes: 420 };

describe('the tickets of a replacement', () => {
  it('are their keys, those not seen said so', () => {
    expect(ticketsText([{ issueId: 'i1', issueKey: 'PLAT-1' }, { issueId: null, issueKey: null }], 'a hidden ticket')).toBe('PLAT-1, a hidden ticket');
    expect(ticketsText([], 'x')).toBe('');
  });
});

describe('ReplaceDialog', () => {
  async function open(role: Session['account']['tenantRole'], shown: Resource = RUST) {
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
    const reply = async (match: (url: string, method: string) => boolean, body: object | null, status = 200): Promise<TestRequest> => {
      let request: TestRequest | undefined;
      for (let i = 0; i < 50 && !request; i++) {
        [request] = http.match((r) => match(r.urlWithParams, r.method));
        if (!request) await new Promise((resolve) => setTimeout(resolve, 20));
      }
      if (!request) throw new Error(`no matching request among ${http.match(() => true).map((r) => r.request.urlWithParams).join(', ') || 'none'}`);
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
    const dialog = () => Array.from(root().querySelectorAll('mat-dialog-container')).at(-1) ?? root();
    // The session the guard loads before the page.
    const loaded = TestBed.inject(AuthService).load();
    const session: Session = {
      account: { id: 'me', email: 'me@acme.example', displayName: 'Me', tenantRole: role },
      csrfToken: 'csrf',
      expiresAt: '2026-12-01T00:00:00Z',
      stage: 'complete',
    };
    await reply((url) => url === '/api/v1/auth/session', session);
    await loaded;
    const fixture = TestBed.createComponent(ResourcePage);
    fixture.componentRef.setInput('id', shown.id);
    await settle();
    await reply((url) => url === `/api/v1/resources/${shown.id}`, shown);
    const page = fixture.nativeElement as HTMLElement;
    /** Opens the dialog and chooses Nathan Roux among the people. */
    const choose = async () => {
      await click(page.querySelector('button.replace'));
      const people = await reply((url) => url.startsWith('/api/v1/resources?'), { items: [NATHAN], nextCursor: null, canManage: true });
      expect(people.request.params.get('kind')).toBe('person');
      expect(people.request.params.get('active')).toBe('true');
      const input = dialog().querySelector<HTMLInputElement>('input.person-input');
      input?.focus();
      input?.dispatchEvent(new Event('focusin'));
      await settle();
      const panel = Array.from(root().querySelectorAll('.mat-mdc-autocomplete-panel')).at(-1);
      await click(Array.from(panel?.querySelectorAll('mat-option') ?? []).find((o) => o.textContent?.includes('Nathan')) ?? null);
    };
    return { http, settle, reply, root, page, text, click, dialog, fixture, choose };
  }

  afterEach(async () => {
    // Closed before the injector goes: closing gives the focus back to the page.
    TestBed.inject(MatDialog).closeAll();
    await new Promise((resolve) => setTimeout(resolve, 200));
    TestBed.tick();
  });

  it('is offered on generic resources to those who manage resources only', async () => {
    const { page } = await open('resource_manager', NATHAN);
    expect(page.querySelector('button.replace')).toBeNull();
    TestBed.resetTestingModule();
    const member = await open('member');
    expect(member.page.querySelector('button.replace')).toBeNull();
  });

  it('previews what moves, then replaces and deactivates the generic resource by default', async () => {
    const { root, page, text, click, reply, dialog, choose, fixture } = await open('resource_manager');
    expect(text(page.querySelector('button.replace'))).toBe('person_add Replace with a person');
    await choose();
    expect(text(dialog().querySelector('h2'))).toBe('Replace Rust developer with a person');
    expect(text(dialog().querySelector('.intro'))).toMatch(/^Its assignments move to the person from today on \(.+\)\. Days before today stay on Rust developer\.$/);
    expect(dialog().querySelector<HTMLInputElement>('mat-checkbox input')?.checked).toBe(true);
    expect(text(dialog().querySelector('mat-checkbox label'))).toBe('Deactivate Rust developer afterwards');

    const asked = await reply((url, method) => url === PREVIEW && method === 'POST', told({ archived: [{ issueId: 'i9', issueKey: 'OLD-3' }] }));
    expect(asked.request.body).toEqual({ personId: NATHAN.id });
    expect(text(dialog().querySelector('#replace-preview'))).toBe('Assignments that move (2)');
    expect(Array.from(dialog().querySelectorAll('.moves li')).map((li) => text(li))).toEqual([
      'PLAT-1 · from Mar 11, 2030 to Mar 15, 2030',
      'A ticket you do not see · from Mar 18, 2030 to Mar 19, 2030 · joins their assignment',
    ]);
    expect(text(dialog().querySelector('.archived'))).toBe('Left on Rust developer, archived projects: OLD-3.');
    expect(dialog().querySelector('.message.overload')).toBeNull();
    expect(await axeViolations(dialog())).toEqual([]);

    await click(dialog().querySelector('button.confirm'));
    const done = await reply((url, method) => url === REPLACE && method === 'POST', told({ deactivated: true }));
    expect(done.request.body).toEqual({ personId: NATHAN.id, deactivate: true, force: false });
    await new Promise((resolve) => setTimeout(resolve, 200));
    await reply((url) => url === `/api/v1/resources/${RUST.id}`, { ...RUST, active: false, version: 2 });
    fixture.detectChanges();
    expect(text(root().querySelector('.message.info[role="status"]'))).toBe(
      'The assignments of Rust developer moved to Nathan Roux (2); Rust developer is now inactive.',
    );
  });

  it('keeps the generic resource active on demand, and says when nothing moves', async () => {
    const { root, text, click, reply, dialog, choose } = await open('tenant_admin');
    await choose();
    await reply((url, method) => url === PREVIEW && method === 'POST', told({ moved: [] }));
    expect(text(dialog().querySelector('.nothing'))).toBe('No assignment of Rust developer from today on: nothing moves.');
    await click(dialog().querySelector('mat-checkbox input'));
    await click(dialog().querySelector('button.confirm'));
    const done = await reply((url, method) => url === REPLACE && method === 'POST', told({ moved: [] }));
    expect(done.request.body).toEqual({ personId: NATHAN.id, deactivate: false, force: false });
    await new Promise((resolve) => setTimeout(resolve, 200));
    await reply((url) => url === `/api/v1/resources/${RUST.id}`, RUST);
    expect(text(root().querySelector('.message.info[role="status"]'))).toBe('The assignments of Rust developer moved to Nathan Roux (0).');
  });

  it('blocks on overlapping segments, naming the tickets', async () => {
    const { text, reply, dialog, choose } = await open('resource_manager');
    await choose();
    await reply(
      (url, method) => url === PREVIEW && method === 'POST',
      told({ overlaps: [{ issueId: 'i2', issueKey: 'PLAT-2' }, { issueId: null, issueKey: null }], blocked: true }),
    );
    expect(text(dialog().querySelector('.overlaps[role="alert"]'))).toBe(
      'Their segments and the moved ones take a same day on PLAT-2, A ticket you do not see. Change one of them before replacing.',
    );
    expect(dialog().querySelector<HTMLButtonElement>('button.confirm')?.disabled).toBe(true);
    expect(dialog().querySelector('button.force')).toBeNull();
  });

  it('blocks an overload, forcing it for those allowed', async () => {
    const { text, click, reply, dialog, choose } = await open('tenant_admin');
    await choose();
    await reply((url, method) => url === PREVIEW && method === 'POST', told({ overloads: [OVER], blocked: true, forceable: true }));
    expect(text(dialog().querySelector('.message.overload[role="alert"]'))).toBe(
      'block Capacity exceeded — Mon, Mar 11: 9 h planned for 7 h available (+2 h). The replacement cannot be made as it is, unless you force the overload.',
    );
    expect(dialog().querySelector<HTMLButtonElement>('button.confirm')?.disabled).toBe(true);
    expect(text(dialog().querySelector('.force-note'))).toContain('reserved for roles with this permission');
    expect(await axeViolations(dialog())).toEqual([]);
    await click(dialog().querySelector('button.force'));
    const forced = await reply((url, method) => url === REPLACE && method === 'POST', told({ overloads: [OVER], deactivated: true }));
    expect(forced.request.body).toEqual({ personId: NATHAN.id, deactivate: true, force: true });
  });

  it('tells a refusal: an overload without the permission, a person no longer active', async () => {
    const { text, click, reply, dialog, choose } = await open('resource_manager');
    await choose();
    await reply((url, method) => url === PREVIEW && method === 'POST', told({ mode: 'warning' }));
    await click(dialog().querySelector('button.confirm'));
    await reply(
      (url, method) => url === REPLACE && method === 'POST',
      { type: 'urn:mlbpp:problem:workload.overload', title: 'x', status: 409, code: 'workload.overload', overload: { days: [OVER], forceable: false } },
      409,
    );
    expect(text(dialog().querySelector('.message.overload[role="alert"]'))).toBe(
      'block Capacity exceeded — Mon, Mar 11: 9 h planned for 7 h available (+2 h). The replacement cannot be made as it is.',
    );
    expect(dialog().querySelector('button.force')).toBeNull();
    expect(dialog().querySelector<HTMLButtonElement>('button.confirm')?.disabled).toBe(true);

    // Choosing again answers it; a person deactivated meanwhile lands on the field.
    await choose();
    await reply((url, method) => url === PREVIEW && method === 'POST', told());
    expect(dialog().querySelector('.message.overload')).toBeNull();
    await click(dialog().querySelector('button.confirm'));
    await reply(
      (url, method) => url === REPLACE && method === 'POST',
      { type: 'urn:mlbpp:problem:validation_failed', title: 'x', status: 400, code: 'validation_failed', errors: [{ field: 'personId', code: 'inactive' }] },
      400,
    );
    expect(text(dialog().querySelector('mat-error'))).toBeTruthy();
  });
});
