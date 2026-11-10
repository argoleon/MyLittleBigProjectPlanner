/**
 * Integration tests of the two-step verification screens, with the API
 * mocked at the HTTP level and the real translations.
 */
import { Clipboard } from '@angular/cdk/clipboard';
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { Component } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { Router, provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { MfaDialog } from '../layout/mfa-dialog';
import { EnrollPage } from './enroll-page';
import { VerifyPage } from './verify-page';

@Component({ template: '<h1>Elsewhere</h1>' })
class Elsewhere {}

const SESSION = {
  account: {
    id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a8c',
    email: 'lucas.bernard@acme.example',
    displayName: 'Lucas Bernard',
    locale: null,
    tenantRole: 'member',
  },
  stage: 'complete',
  csrfToken: 'csrf-token',
  expiresAt: '2026-11-17T09:00:00Z',
};

const SETUP = {
  otpauthUri: 'otpauth://totp/MLBPP:lucas?secret=JBSWY3DPEHPK3PXP',
  secret: 'JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP',
  qrCodeSvg: '<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>',
};

const CODES = ['abcde-fghjk', 'mnpqr-stuvw', 'xyz23-45678', 'abcde-23456', 'fghjk-mnpqr', 'stuvw-xyz23', 'hjkmn-pqrst', 'uvwxy-z2345'];

function problem(status: number, code: string) {
  return { type: `urn:mlbpp:problem:${code}`, title: code, status, code };
}

async function setUp(url: string) {
  localStorage.clear();
  TestBed.configureTestingModule({
    providers: [
      provideRouter(
        [
          { path: 'sign-in/verify', component: VerifyPage },
          { path: 'sign-in/enroll', component: EnrollPage },
          { path: '**', component: Elsewhere },
        ],
        withComponentInputBinding(),
      ),
      provideApi(),
      provideHttpClientTesting(),
      provideI18nTesting(),
      provideMaterialDefaults(),
    ],
  });
  const harness = await RouterTestingHarness.create();
  await harness.navigateByUrl(url);
  const http = TestBed.inject(HttpTestingController);
  const root = () => document.body;
  // Not `whenStable`: a pending resource (the dialog's status) keeps the app
  // unstable until its request is answered.
  const settle = async () => {
    for (let i = 0; i < 3; i++) {
      await new Promise((resolve) => setTimeout(resolve));
      TestBed.tick();
    }
  };
  const reply = async (url: string, body: object | null, status = 200): Promise<TestRequest> => {
    let request: TestRequest | undefined;
    // Not `whenStable` while waiting: a pending resource keeps the app unstable.
    for (let i = 0; i < 20 && !request; i++) {
      [request] = http.match(url);
      if (!request) await new Promise((resolve) => setTimeout(resolve));
    }
    if (!request) throw new Error(`no request to ${url}`);
    request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error' });
    await settle();
    await settle();
    return request;
  };
  const fill = (name: string, value: string) => {
    const input = root().querySelector(`input[formcontrolname="${name}"]`) as HTMLInputElement;
    input.value = value;
    input.dispatchEvent(new Event('input'));
  };
  const click = async (selector: string) => {
    (root().querySelector(selector) as HTMLElement).click();
    await settle();
  };
  const text = (selector: string) => root().querySelector(selector)?.textContent?.replace(/\s+/g, ' ').trim();
  return { harness, http, reply, fill, click, text, settle, root };
}

afterEach(() => {
  TestBed.inject(HttpTestingController).verify();
  TestBed.inject(MatDialog).closeAll();
});

describe('VerifyPage', () => {
  it('completes sign-in with a code, then goes where the member was going', async () => {
    const { reply, fill, click, text, harness } = await setUp('/sign-in/verify?returnUrl=%2Fgantt');
    const element = harness.routeNativeElement as HTMLElement;
    expect(text('.eyebrow')).toBe('Step 2 of 2');
    expect(element.querySelector('h1')?.textContent).toBe('Two-step verification');
    const input = element.querySelector('input') as HTMLInputElement;
    expect(input.getAttribute('autocomplete')).toBe('one-time-code');
    expect(input.getAttribute('inputmode')).toBe('numeric');
    expect(await axeViolations(element)).toEqual([]);

    fill('code', '000000');
    await click('button[type="submit"]');
    await reply('/api/v1/auth/sign-in/verify', problem(401, 'auth.invalid_code'), 401);
    expect(text('[role="alert"]')).toBe('Incorrect or already used code.');

    fill('code', '482913');
    await click('button[type="submit"]');
    const request = await reply('/api/v1/auth/sign-in/verify', SESSION);
    expect(request.request.body).toEqual({ code: '482913' });
    expect(TestBed.inject(Router).url).toBe('/gantt');
  });

  it('accepts a backup code instead', async () => {
    const { reply, fill, click, text, harness } = await setUp('/sign-in/verify');
    await click('button.switch');
    expect(text('mat-label')).toBe('Backup code');
    expect(text('.subtitle')).toContain('backup codes');
    expect((harness.routeNativeElement as HTMLElement).querySelector('input')?.getAttribute('inputmode')).toBeNull();
    fill('code', 'abcde-fghjk');
    await click('button[type="submit"]');
    const request = await reply('/api/v1/auth/sign-in/verify', SESSION);
    expect(request.request.body).toEqual({ code: 'abcde-fghjk' });
    expect(TestBed.inject(Router).url).toBe('/');
  });

  it('explains rate limiting', async () => {
    const { reply, fill, click, text } = await setUp('/sign-in/verify');
    fill('code', '123456');
    await click('button[type="submit"]');
    await reply('/api/v1/auth/sign-in/verify', problem(429, 'too_many_requests'), 429);
    expect(text('[role="alert"]')).toBe('Too many attempts. Try again in a few minutes.');
  });
});

describe('EnrollPage and the enrollment steps', () => {
  it('scan, confirm, keep the backup codes, then continue', async () => {
    const { reply, fill, click, text, root } = await setUp('/sign-in/enroll?returnUrl=%2Fboards');
    expect(text('h1')).toBe('Enable two-step verification');
    await reply('/api/v1/account/mfa/setup', SETUP);

    const qr = root().querySelector('img.qr') as HTMLImageElement;
    expect(qr.getAttribute('src')).toMatch(/^data:image\/svg\+xml;base64,/);
    expect(qr.getAttribute('alt')).toContain('QR code');
    expect(text('code.key')).toBe('JBSW Y3DP EHPK 3PXP JBSW Y3DP EHPK 3PXP');
    expect(await axeViolations(root().querySelector('main')!)).toEqual([]);

    fill('code', '12');
    await click('form button[type="submit"]');
    expect(text('mat-error')).toBe('The format is invalid.');

    fill('code', '000000');
    await click('form button[type="submit"]');
    await reply('/api/v1/account/mfa/confirm', problem(401, 'auth.invalid_code'), 401);
    expect(text('[role="alert"]')).toBe('Incorrect or already used code.');

    fill('code', '482 913');
    await click('form button[type="submit"]');
    const confirm = await reply('/api/v1/account/mfa/confirm', { backupCodes: CODES, session: SESSION });
    expect(confirm.request.body).toEqual({ code: '482 913' });
    expect(text('[role="status"]')).toBe('Two-step verification is enabled.');
    expect(Array.from(root().querySelectorAll('ol.codes li')).map((li) => li.textContent?.trim())).toEqual(CODES);
    expect(await axeViolations(root().querySelector('main')!)).toEqual([]);

    const copy = vi.spyOn(TestBed.inject(Clipboard), 'copy').mockReturnValue(true);
    const buttons = () => Array.from(root().querySelectorAll<HTMLButtonElement>('.actions button'));
    buttons()[0].click();
    await new Promise((resolve) => setTimeout(resolve));
    expect(copy).toHaveBeenCalledWith(CODES.join('\n'));

    buttons()[2].click();
    await new Promise((resolve) => setTimeout(resolve, 10));
    expect(TestBed.inject(Router).url).toBe('/boards');
  });

  it('offers to retry when no secret could be drawn', async () => {
    const { reply, click, text } = await setUp('/sign-in/enroll');
    await reply('/api/v1/account/mfa/setup', problem(500, 'internal'), 500);
    expect(text('[role="alert"]')).toBe('The server ran into an error. Try again later.');
    await click('app-mfa-enrollment button');
    await reply('/api/v1/account/mfa/setup', SETUP);
    expect(text('code.key')).toContain('JBSW');
  });
});

describe('MfaDialog', () => {
  async function open(status: object) {
    const page = await setUp('/elsewhere');
    TestBed.inject(MatDialog).open(MfaDialog);
    await page.reply('/api/v1/account/mfa', status);
    return page;
  }

  it('enables two-step verification when it is off', async () => {
    const { reply, text } = await open({ enabled: false, backupCodesLeft: 0, requiredByPolicy: false });
    expect(text('h2')).toBe('Two-step verification');
    await reply('/api/v1/account/mfa/setup', SETUP);
    expect(document.querySelector('mat-dialog-container img.qr')).not.toBeNull();
  });

  it('disables it with a code when the organization allows it', async () => {
    const { reply, fill, click, text } = await open({ enabled: true, backupCodesLeft: 6, requiredByPolicy: false });
    expect(text('[role="status"]')).toBe('Two-step verification is enabled. Backup codes left: 6.');
    expect(await axeViolations(document.querySelector('mat-dialog-container')!)).toEqual([]);

    fill('code', 'abcde-fghjk');
    await click('mat-dialog-container form button[type="submit"]');
    const request = await reply('/api/v1/account/mfa/disable', null, 204);
    expect(request.request.body).toEqual({ code: 'abcde-fghjk' });
    await reply('/api/v1/account/mfa', { enabled: false, backupCodesLeft: 0, requiredByPolicy: false });
    await reply('/api/v1/account/mfa/setup', SETUP);
  });

  it('cannot disable what the organization requires', async () => {
    const { text, root } = await open({ enabled: true, backupCodesLeft: 8, requiredByPolicy: true });
    expect(text('mat-dialog-content')).toContain('Your organization requires it: it cannot be disabled.');
    expect(root().querySelector('mat-dialog-container form')).toBeNull();
  });
});
