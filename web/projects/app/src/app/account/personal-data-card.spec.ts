/**
 * Integration tests of "My personal data": the export and the erasure
 * dialog, with the API mocked at the HTTP level and the real translations.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { Component, signal } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { FormControl } from '@angular/forms';
import { MatDialog } from '@angular/material/dialog';
import { Router, provideRouter } from '@angular/router';
import { Me, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { confirmsEmail, emailConfirmation } from './erasure-dialog';
import { EXPORT_POLL_MS, PersonalDataCard, SAVE_FILE, exportFileName } from './personal-data-card';

const ME: Me = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a01',
  email: 'lucas.bernard@acme.example',
  displayName: 'Lucas Bernard',
  locale: null,
  timeZone: null,
  hasPassword: true,
  tenantId: 't1',
  tenantRole: 'member',
  accountType: 'internal',
};

const EXPORT_ID = 'Rk9vYmFyLWV4cG9ydC1pZC0xMjM0NTY3ODkwYWJjZGVm';

function problem(status: number, code: string, errors: object[] = []) {
  return { type: `urn:mlbpp:problem:${code}`, title: code, status, code, errors };
}

@Component({
  imports: [PersonalDataCard],
  template: `<app-personal-data-card [me]="me()" />`,
})
class Host {
  readonly me = signal<Me | null>(ME);
}

async function open(me: Me = ME) {
  localStorage.clear();
  const downloads: { name: string; content: Blob }[] = [];
  TestBed.configureTestingModule({
    providers: [
      provideRouter([]),
      provideApi(),
      provideHttpClientTesting(),
      provideI18nTesting(),
      provideMaterialDefaults(),
      { provide: SAVE_FILE, useValue: (name: string, content: Blob) => downloads.push({ name, content }) },
      { provide: EXPORT_POLL_MS, useValue: 5 },
    ],
  });
  const fixture = TestBed.createComponent(Host);
  fixture.componentInstance.me.set(me);
  await fixture.whenStable();
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
  const card = () => fixture.nativeElement as HTMLElement;
  const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
  const click = async (element: Element | null) => {
    (element as HTMLElement).click();
    await settle();
  };
  const dialog = () => Array.from(document.querySelectorAll('mat-dialog-container')).at(-1)!;
  const type = async (name: string, value: string) => {
    const input = dialog().querySelector(`input[formcontrolname="${name}"]`) as HTMLInputElement;
    input.value = value;
    input.dispatchEvent(new Event('input'));
    input.dispatchEvent(new Event('blur'));
    await settle();
  };
  return { http, reply, settle, card, text, click, dialog, type, downloads };
}

describe('exportFileName', () => {
  it('dates the file in the user’s time zone', () => {
    const lateEvening = new Date('2026-11-11T23:30:00Z');
    expect(exportFileName(lateEvening, 'UTC')).toBe('personal-data-2026-11-11.json');
    expect(exportFileName(lateEvening, 'Europe/Paris')).toBe('personal-data-2026-11-12.json');
  });
});

describe('email confirmation', () => {
  it('accepts the email whatever its case and spaces', () => {
    expect(confirmsEmail(' Lucas.Bernard@ACME.example ', 'lucas.bernard@acme.example')).toBe(true);
    expect(confirmsEmail('lucas.bernard@acme.exampl', 'lucas.bernard@acme.example')).toBe(false);
    expect(confirmsEmail('', '')).toBe(false);
  });

  it('validates a control, leaving an empty one to `required`', () => {
    const validator = emailConfirmation('lucas.bernard@acme.example');
    expect(validator(new FormControl(''))).toBeNull();
    expect(validator(new FormControl('someone@else.test'))).toEqual({ mismatch: true });
    expect(validator(new FormControl('LUCAS.BERNARD@acme.example'))).toBeNull();
  });
});

describe('PersonalDataCard', () => {
  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify();
  });

  it('explains erasure and offers the export and the deletion', async () => {
    const { card, text } = await open();
    expect(text(card().querySelector('h2'))).toBe('My personal data');
    expect(text(card().querySelector('.intro'))).toContain('anonymizes your name and email everywhere');
    expect(text(card().querySelector('button.export'))).toBe('download Export (JSON)');
    expect(text(card().querySelector('button.erase'))).toBe('Delete my account');
    expect(await axeViolations(card())).toEqual([]);
  });

  it('exports the data, follows the export, then downloads it', async () => {
    const { reply, card, text, click, downloads } = await open();
    await click(card().querySelector('button.export'));
    expect(card().querySelector('mat-progress-bar')).not.toBeNull();
    expect((card().querySelector('button.export') as HTMLButtonElement).disabled).toBe(true);
    const requested = await reply((url, method) => url === '/api/v1/me/exports' && method === 'POST', {
      id: EXPORT_ID,
      status: 'pending',
      expiresAt: '2026-11-11T10:15:00Z',
    }, 202);
    expect(requested.request.method).toBe('POST');
    const follow = (url: string) => url === `/api/v1/me/exports/${EXPORT_ID}`;
    await reply(follow, { id: EXPORT_ID, status: 'pending', expiresAt: '2026-11-11T10:15:00Z' });
    expect(downloads).toEqual([]);
    await reply(follow, { id: EXPORT_ID, status: 'ready', expiresAt: '2026-11-11T10:15:00Z' });
    expect(downloads).toEqual([]);
    const data = { format: 'mlbpp.personal-data.v1', account: { email: ME.email } };
    await reply((url) => url === `/api/v1/me/exports/${EXPORT_ID}/file`, data);
    expect(downloads.length).toBe(1);
    expect(downloads[0].name).toMatch(/^personal-data-\d{4}-\d{2}-\d{2}\.json$/);
    expect(downloads[0].content.type).toBe('application/json');
    expect(JSON.parse(await downloads[0].content.text())).toEqual(data);
    expect(text(card().querySelector('[role="status"]'))).toBe('Your export is downloaded.');
    expect(card().querySelector('mat-progress-bar')).toBeNull();
  });

  it('says why an export is refused', async () => {
    const { reply, card, text, click, downloads } = await open();
    await click(card().querySelector('button.export'));
    await reply((url) => url === '/api/v1/me/exports', problem(429, 'too_many_requests'), 429);
    expect(text(card().querySelector('[role="alert"]'))).toBe('Too many attempts. Try again in a few minutes.');
    expect(downloads).toEqual([]);
  });

  it('erases the account with the password and the email typed, then signs out', async () => {
    const { reply, card, text, click, dialog, type } = await open();
    const router = TestBed.inject(Router);
    const navigate = vi.spyOn(router, 'navigate').mockResolvedValue(true);
    await click(card().querySelector('button.erase'));
    expect(text(dialog().querySelector('h2'))).toBe('Delete your account?');
    expect(dialog().querySelectorAll('.consequences li').length).toBe(4);
    expect(text(dialog().querySelector('mat-label'))).toBe('Your password');
    expect(await axeViolations(dialog())).toEqual([]);

    // Nothing sent until the form holds.
    await click(dialog().querySelector('button.confirm'));
    expect(dialog().querySelectorAll('mat-error').length).toBe(2);
    await type('password', 'not mine');
    await type('confirmation', 'someone@else.test');
    expect(text(dialog().querySelectorAll('mat-error')[0])).toBe('This is not the email of your account.');

    await type('confirmation', 'Lucas.Bernard@acme.example');
    await click(dialog().querySelector('button.confirm'));
    await reply(
      (url) => url === '/api/v1/me/erasure',
      problem(400, 'validation_failed', [{ field: 'password', code: 'incorrect' }]),
      400,
    );
    expect(text(dialog().querySelector('mat-error'))).toBe('This is not your current password.');

    await type('password', 'correct horse battery staple');
    await click(dialog().querySelector('mat-checkbox input'));
    await click(dialog().querySelector('button.confirm'));
    const erased = await reply((url) => url === '/api/v1/me/erasure', null, 204);
    expect(erased.request.body).toEqual({
      confirmation: 'Lucas.Bernard@acme.example',
      purgeComments: true,
      password: 'correct horse battery staple',
    });
    // Once the dialog is gone.
    for (let i = 0; i < 50 && navigate.mock.calls.length === 0; i++) {
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    expect(navigate).toHaveBeenCalledWith(['/sign-in'], { queryParams: { accountErased: 'true' } });
  });

  it('asks an account without password to sign in again through its organization', async () => {
    const { reply, card, text, click, dialog, type } = await open({ ...ME, hasPassword: false });
    await click(card().querySelector('button.erase'));
    expect(dialog().querySelector('input[formcontrolname="password"]')).toBeNull();
    expect(text(dialog().querySelector('.hint'))).toContain('sign in again just before');
    await type('confirmation', ME.email);
    await click(dialog().querySelector('button.confirm'));
    const refused = await reply((url) => url === '/api/v1/me/erasure', problem(403, 'account.reauthentication_required'), 403);
    expect(refused.request.body).toEqual({ confirmation: ME.email, purgeComments: false });
    expect(text(dialog().querySelector('[role="alert"]'))).toBe(
      'Sign in again through your organization, then delete your account within 10 minutes.',
    );
  });

  it('cannot open the deletion before the account is known', async () => {
    const { card } = await open();
    TestBed.inject(MatDialog).closeAll();
    const host = TestBed.createComponent(Host);
    host.componentInstance.me.set(null);
    await host.whenStable();
    expect(((host.nativeElement as HTMLElement).querySelector('button.erase') as HTMLButtonElement).disabled).toBe(true);
    expect(card()).toBeTruthy();
  });
});
