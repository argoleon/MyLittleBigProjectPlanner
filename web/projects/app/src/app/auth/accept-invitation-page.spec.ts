/**
 * Integration tests of the invitation page, with the API mocked at the HTTP
 * level and the real translations.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { Router, provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { AcceptInvitationPage } from './accept-invitation-page';

const PREVIEW = {
  email: 'camille.martin@acme.example',
  tenantName: 'Acme Industrie',
  invitedBy: 'Lucas Bernard',
  accountType: 'internal',
  newAccount: true,
  expiresAt: '2026-11-18T09:00:00Z',
};

const SESSION = {
  account: {
    id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a8c',
    email: 'camille.martin@acme.example',
    displayName: 'Camille Martin',
    locale: null,
    tenantRole: 'member',
  },
  stage: 'complete',
  csrfToken: 'csrf-token',
  expiresAt: '2026-11-17T09:00:00Z',
};

async function open(url = '/accept-invitation?token=t1') {
  localStorage.clear();
  TestBed.configureTestingModule({
    providers: [
      provideRouter(
        [
          { path: 'accept-invitation', component: AcceptInvitationPage },
          { path: '**', children: [] },
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
  const settle = async () => {
    for (let i = 0; i < 3; i++) {
      await new Promise((resolve) => setTimeout(resolve));
      TestBed.tick();
    }
  };
  const reply = async (path: string, body: object | null, status = 200): Promise<TestRequest> => {
    let request: TestRequest | undefined;
    for (let i = 0; i < 50 && !request; i++) {
      [request] = http.match(path);
      if (!request) await new Promise((resolve) => setTimeout(resolve, 20));
    }
    if (!request) throw new Error(`no request to ${path}`);
    request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error' });
    await settle();
    return request;
  };
  const root = () => harness.routeNativeElement!;
  const text = (selector: string) => root().querySelector(selector)?.textContent?.replace(/\s+/g, ' ').trim();
  const fill = (name: string, value: string) => {
    const input = root().querySelector(`input[formcontrolname="${name}"]`) as HTMLInputElement;
    input.value = value;
    input.dispatchEvent(new Event('input'));
  };
  const submit = async () => {
    (root().querySelector('button[type="submit"]') as HTMLButtonElement).click();
    await settle();
  };
  return { reply, root, text, fill, submit, settle };
}

function problem(status: number, code: string, errors: object[] = []) {
  return { type: `urn:mlbpp:problem:${code}`, title: code, status, code, errors };
}

describe('AcceptInvitationPage', () => {
  afterEach(() => TestBed.inject(HttpTestingController).verify());

  it('creates the account, then lands in the application', async () => {
    const { reply, root, text, fill, submit } = await open();
    const shown = await reply('/api/v1/auth/invitation', PREVIEW);
    expect(shown.request.body).toEqual({ token: 't1' });
    expect(text('.subtitle')).toBe('Lucas Bernard invites you to join Acme Industrie.');
    expect((root().querySelector('input[readonly]') as HTMLInputElement).value).toBe(PREVIEW.email);
    expect(await axeViolations(root())).toEqual([]);

    // Every field is required, and the confirmation must match.
    await submit();
    expect(root().querySelectorAll('mat-error').length).toBe(3);
    fill('displayName', 'Camille Martin');
    fill('password', 'a long enough passphrase');
    fill('confirmation', 'a long enough passphrase');
    await submit();
    const accepted = await reply('/api/v1/auth/invitation/accept', {
      accountType: 'internal',
      ssoRequired: false,
      session: SESSION,
    });
    expect(accepted.request.body).toEqual({
      token: 't1',
      displayName: 'Camille Martin',
      password: 'a long enough passphrase',
    });
    expect(TestBed.inject(Router).url).toBe('/');
  });

  it('lets an existing account join as it is, and goes to the second factor', async () => {
    const { reply, text, submit } = await open();
    await reply('/api/v1/auth/invitation', { ...PREVIEW, newAccount: false });
    expect(text('.hint')).toContain('You already have an account with this email');
    await submit();
    const accepted = await reply('/api/v1/auth/invitation/accept', {
      accountType: 'internal',
      ssoRequired: false,
      session: { ...SESSION, stage: 'mfa_challenge' },
    });
    expect(accepted.request.body).toEqual({ token: 't1' });
    expect(TestBed.inject(Router).url).toBe('/sign-in/verify');
  });

  it('tells an external member where to go next', async () => {
    const { reply, text, submit } = await open();
    await reply('/api/v1/auth/invitation', { ...PREVIEW, newAccount: false, accountType: 'external' });
    await submit();
    await reply('/api/v1/auth/invitation/accept', { accountType: 'external', ssoRequired: false, session: null });
    expect(text('[role="status"]')).toBe('Your account is ready. You will reach Acme Industrie through its request portal.');
  });

  it('sends a member of an SSO organization to its sign-in page', async () => {
    const { reply, text, root, submit } = await open();
    await reply('/api/v1/auth/invitation', { ...PREVIEW, newAccount: false });
    await submit();
    await reply('/api/v1/auth/invitation/accept', { accountType: 'internal', ssoRequired: true, session: null });
    expect(text('[role="status"]')).toBe('You joined Acme Industrie. It signs you in through its identity provider.');
    expect(root().querySelector('a.link')?.getAttribute('href')).toBe('/sign-in');
  });

  it('shows the API’s field errors', async () => {
    const { reply, text, fill, submit } = await open();
    await reply('/api/v1/auth/invitation', PREVIEW);
    fill('displayName', 'Camille Martin');
    fill('password', 'camille.martin@acme.example');
    fill('confirmation', 'camille.martin@acme.example');
    await submit();
    await reply(
      '/api/v1/auth/invitation/accept',
      problem(400, 'validation_failed', [{ field: 'password', code: 'same_as_email' }]),
      400,
    );
    expect(text('mat-error')).toBe('The password must differ from the email.');
  });

  it('says when the link is invalid or incomplete', async () => {
    const { reply, text } = await open();
    await reply('/api/v1/auth/invitation', problem(400, 'invitation.invalid'), 400);
    expect(text('[role="alert"]')).toBe('This invitation link is invalid, has expired or was already used. Ask for a new one.');
    TestBed.resetTestingModule();
    const incomplete = await open('/accept-invitation');
    expect(incomplete.text('[role="alert"]')).toBe('This invitation link is incomplete. Open the link of the email again.');
  });
});
