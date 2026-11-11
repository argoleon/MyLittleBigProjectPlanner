/**
 * Integration tests of My account, with the API mocked at the HTTP level
 * and the real translations.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { AccountSession, Me, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { AccountPage } from './account-page';
import { deviceName } from './sessions-dialog';

const ME: Me = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a01',
  email: 'lucas.bernard@acme.example',
  displayName: 'Lucas Bernard',
  locale: null,
  timeZone: null,
  tenantId: 't1',
  tenantRole: 'tenant_admin',
  accountType: 'internal',
};

const TENANT = { id: 't1', slug: 'acme', name: 'Acme Industrie', defaultLocale: 'fr', timeZone: 'Europe/Paris' };

const MEMBERSHIPS = [
  { tenantId: 't1', slug: 'acme', name: 'Acme Industrie', url: 'https://acme.example', accountType: 'internal', tenantRole: 'tenant_admin', current: true },
  { tenantId: 't2', slug: 'services', name: 'Acme Services', url: 'https://services.example', accountType: 'internal', tenantRole: 'member', current: false },
];

const THIS: AccountSession = {
  id: 'a'.repeat(32),
  current: true,
  tenantId: 't1',
  tenantName: 'Acme Industrie',
  browser: 'Firefox',
  os: 'Linux',
  createdAt: '2026-11-11T08:00:00Z',
  lastSeenAt: '2026-11-11T10:00:00Z',
  expiresAt: '2026-11-18T08:00:00Z',
};

const PHONE: AccountSession = {
  ...THIS,
  id: 'b'.repeat(32),
  current: false,
  tenantId: 't2',
  tenantName: 'Acme Services',
  browser: 'Safari',
  os: 'iOS',
  createdAt: '2026-11-10T07:00:00Z',
  lastSeenAt: '2026-11-10T19:00:00Z',
};

const MFA_ON = { enabled: true, backupCodesLeft: 8, requiredByPolicy: false };

function problem(status: number, code: string, errors: object[] = []) {
  return { type: `urn:mlbpp:problem:${code}`, title: code, status, code, errors };
}

async function open({ mfa = MFA_ON as object, sessions = [THIS, PHONE] } = {}) {
  localStorage.clear();
  TestBed.configureTestingModule({
    providers: [
      provideRouter([{ path: 'account', component: AccountPage }]),
      provideApi(),
      provideHttpClientTesting(),
      provideI18nTesting(),
      provideMaterialDefaults(),
    ],
  });
  const harness = await RouterTestingHarness.create();
  await harness.navigateByUrl('/account');
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
  const get = (path: string) => (url: string, method: string) => url === path && method === 'GET';
  await reply(get('/api/v1/me'), ME);
  await reply(get('/api/v1/tenant'), TENANT);
  await reply(get('/api/v1/account/mfa'), mfa);
  await reply(get('/api/v1/me/sessions'), sessions);
  await reply(get('/api/v1/me/memberships'), MEMBERSHIPS);
  const root = () => document.body;
  const page = () => root().querySelector('app-account-page')!;
  const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
  const click = async (element: Element | null) => {
    (element as HTMLElement).click();
    await settle();
  };
  // The last one opened: a closed one may still be leaving.
  const dialog = () => Array.from(root().querySelectorAll('mat-dialog-container')).at(-1)!;
  const type = async (container: Element, name: string, value: string) => {
    const input = container.querySelector(`input[formcontrolname="${name}"]`) as HTMLInputElement;
    input.value = value;
    input.dispatchEvent(new Event('input'));
    input.dispatchEvent(new Event('blur'));
    await settle();
  };
  const choose = async (selectIndex: number, label: string) => {
    await click(page().querySelectorAll('mat-select')[selectIndex].querySelector('.mat-mdc-select-trigger'));
    await click(Array.from(root().querySelectorAll('mat-option')).find((o) => text(o) === label)!);
  };
  return { http, reply, settle, get, root, page, text, click, dialog, type, choose };
}

describe('deviceName', () => {
  it('names what is known of the device', () => {
    expect(deviceName({ browser: 'Firefox', os: 'Linux' })).toBe('Firefox · Linux');
    expect(deviceName({ browser: null, os: 'iOS' })).toBe('iOS');
    expect(deviceName({ browser: null, os: null })).toBeNull();
  });
});

describe('AccountPage', () => {
  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify();
  });

  it('shows the organizations, the preferences and the security of the account', async () => {
    const { page, text } = await open();
    expect(text(page().querySelector('h1'))).toBe('My account');
    expect(text(page().querySelector('.eyebrow'))).toBe('Global account · lucas.bernard@acme.example');
    const organizations = Array.from(page().querySelectorAll('.organizations li .row-text, .organizations li .current')).map(text);
    expect(organizations).toEqual([
      'Acme Industrie Organization admin · Internal',
      'Current',
      'Acme Services Member · Internal',
    ]);
    const selects = Array.from(page().querySelectorAll('mat-select')).map(text);
    expect(selects).toEqual(['This device’s language', 'The organization’s (Europe/Paris)']);
    expect(text(page().querySelector('#mfa-detail'))).toBe('Authenticator app · backup codes left: 8');
    const mfaSwitch = page().querySelector('mat-slide-toggle button[role="switch"]')!;
    expect(mfaSwitch.getAttribute('aria-checked')).toBe('true');
    expect(mfaSwitch.getAttribute('aria-labelledby')).toBe('mfa-label');
    const sessions = Array.from(page().querySelectorAll('.row')).find((r) => r.textContent?.includes('Active sessions'));
    expect(text(sessions?.querySelector('.row-detail'))).toBe('Devices: 2 · latest sign-in Nov 11, 2026, 9:00 AM');
    expect(await axeViolations(page())).toEqual([]);
  });

  it('applies and saves the language and the time zone at once', async () => {
    const { reply, page, text, choose } = await open();
    await choose(0, 'Français');
    const language = await reply((url, method) => url === '/api/v1/me/preferences' && method === 'PUT', {
      locale: 'fr',
      timeZone: null,
    });
    expect(language.request.body).toEqual({ locale: 'fr', timeZone: null });
    expect(text(page().querySelector('h1'))).toBe('Mon compte');
    expect(text(page().querySelector('[role="status"]'))).toBe('Préférences enregistrées.');

    await choose(1, 'Asia/Tokyo');
    const zone = await reply((url, method) => url === '/api/v1/me/preferences' && method === 'PUT', {
      locale: 'fr',
      timeZone: 'Asia/Tokyo',
    });
    expect(zone.request.body).toEqual({ locale: 'fr', timeZone: 'Asia/Tokyo' });
    // Sign-ins now shown in Tokyo time.
    const sessions = Array.from(page().querySelectorAll('.row')).find((r) => r.textContent?.includes('Sessions actives'));
    expect(text(sessions?.querySelector('.row-detail'))).toContain('17:00');

    await choose(1, 'Celui de l’organisation (Europe/Paris)');
    const reset = await reply((url, method) => url === '/api/v1/me/preferences' && method === 'PUT', {
      locale: 'fr',
      timeZone: null,
    });
    expect(reset.request.body).toEqual({ locale: 'fr', timeZone: null });
  });

  it('says why preferences are refused', async () => {
    const { reply, page, text, choose } = await open();
    await choose(1, 'Asia/Tokyo');
    await reply(
      (url, method) => url === '/api/v1/me/preferences' && method === 'PUT',
      problem(400, 'validation_failed', [{ field: 'timeZone', code: 'unknown_time_zone' }]),
      400,
    );
    expect(text(page().querySelector('[role="alert"]'))).toBe('Some fields are invalid.');
  });

  it('changes the password with the current one', async () => {
    const { reply, get, page, text, click, dialog, type } = await open();
    await click(page().querySelector('button.change-password'));
    expect(text(dialog().querySelector('h2'))).toBe('Change your password');

    await type(dialog(), 'currentPassword', 'not mine');
    await type(dialog(), 'newPassword', 'a much longer passphrase');
    await type(dialog(), 'confirmation', 'a much longer passphrasE');
    await click(dialog().querySelector('button[type="submit"]'));
    expect(text(dialog().querySelector('mat-error'))).toBe('The two passwords differ.');

    await type(dialog(), 'confirmation', 'a much longer passphrase');
    await click(dialog().querySelector('button[type="submit"]'));
    const wrong = await reply(
      (url) => url === '/api/v1/me/password',
      problem(400, 'validation_failed', [{ field: 'currentPassword', code: 'incorrect' }]),
      400,
    );
    expect(wrong.request.body).toEqual({ currentPassword: 'not mine', newPassword: 'a much longer passphrase' });
    expect(text(dialog().querySelector('mat-error'))).toBe('This is not your current password.');

    await type(dialog(), 'currentPassword', 'correct horse battery staple');
    await click(dialog().querySelector('button[type="submit"]'));
    await reply((url) => url === '/api/v1/me/password', null, 204);
    await reply(get('/api/v1/me/sessions'), [THIS]);
    expect(text(page().querySelector('[role="status"]'))).toBe('Password changed. Your other sessions are signed out.');
  });

  it('tells an account of the organization’s directory that it has no password', async () => {
    const { reply, page, click, dialog, type, text } = await open();
    await click(page().querySelector('button.change-password'));
    await type(dialog(), 'currentPassword', 'anything');
    await type(dialog(), 'newPassword', 'a much longer passphrase');
    await type(dialog(), 'confirmation', 'a much longer passphrase');
    await click(dialog().querySelector('button[type="submit"]'));
    await reply((url) => url === '/api/v1/me/password', problem(409, 'account.no_password'), 409);
    expect(text(dialog().querySelector('[role="alert"]'))).toBe(
      'Your organization signs you in: your account has no password to change.',
    );
  });

  it('lists the sessions and signs out another device', async () => {
    const { reply, get, page, text, click, dialog } = await open();
    await click(page().querySelector('button.manage-sessions'));
    await reply(get('/api/v1/me/sessions'), [THIS, PHONE]);
    const rows = Array.from(dialog().querySelectorAll('li'));
    expect(text(rows[0].querySelector('.device'))).toBe('Firefox · Linux This session');
    expect(rows[0].querySelector('button')).toBeNull();
    expect(text(rows[1].querySelector('.device'))).toBe('Safari · iOS');
    expect(text(rows[1].querySelector('.detail'))).toBe('Acme Services · latest activity Nov 10, 2026, 8:00 PM');
    expect(await axeViolations(dialog())).toEqual([]);

    await click(dialog().querySelector('button[aria-label="Sign out the session on Safari · iOS"]'));
    const revoked = await reply((url) => url === `/api/v1/me/sessions/${PHONE.id}`, null, 204);
    expect(revoked.request.method).toBe('DELETE');
    await reply(get('/api/v1/me/sessions'), [THIS]);
    expect(text(dialog().querySelector('[role="status"]'))).toBe('Session signed out.');
    expect(dialog().querySelectorAll('li').length).toBe(1);

    TestBed.inject(MatDialog).closeAll();
    await reply(get('/api/v1/me/sessions'), [THIS]);
  });

  it('opens two-step verification from its switch, which only follows what holds', async () => {
    const { reply, get, page, text, click, dialog } = await open({ mfa: { enabled: false, backupCodesLeft: 0, requiredByPolicy: false } });
    expect(text(page().querySelector('#mfa-detail'))).toBe('Off: your password alone signs you in.');
    const mfaSwitch = page().querySelector('mat-slide-toggle button[role="switch"]')!;
    await click(mfaSwitch);
    expect(mfaSwitch.getAttribute('aria-checked')).toBe('false');
    expect(text(dialog().querySelector('h2'))).toBe('Two-step verification');
    await reply(get('/api/v1/account/mfa'), { enabled: false, backupCodesLeft: 0, requiredByPolicy: false });
    // Not enabled: the dialog starts the enrollment.
    await reply((url) => url === '/api/v1/account/mfa/setup', {
      otpauthUri: 'otpauth://totp/MLBPP:lucas?secret=JBSWY3DPEHPK3PXP',
      secret: 'JBSWY3DPEHPK3PXP',
      qrCodeSvg: '<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"></svg>',
    });
    TestBed.inject(MatDialog).closeAll();
    await reply(get('/api/v1/account/mfa'), MFA_ON);
    expect(mfaSwitch.getAttribute('aria-checked')).toBe('true');
  });
});
