/**
 * Unit tests of the pages before sign-in, with the API mocked at the HTTP
 * level and the real translations.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { Component } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { Router, provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { ForgotPasswordPage } from './forgot-password-page';
import { ResetPasswordPage } from './reset-password-page';
import { SignInPage } from './sign-in-page';

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

function problem(status: number, code: string, errors: object[] = []) {
  return { type: `urn:mlbpp:problem:${code}`, title: code, status, code, errors };
}

async function open(url: string) {
  localStorage.clear();
  TestBed.configureTestingModule({
    providers: [
      provideRouter(
        [
          { path: 'sign-in', component: SignInPage },
          { path: 'forgot-password', component: ForgotPasswordPage },
          { path: 'reset-password', component: ResetPasswordPage },
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
  const element = harness.routeNativeElement as HTMLElement;
  const fill = (name: string, value: string) => {
    const input = element.querySelector(`input[formcontrolname="${name}"]`) as HTMLInputElement;
    input.value = value;
    input.dispatchEvent(new Event('input'));
  };
  const submit = async () => {
    (element.querySelector('button[type="submit"]') as HTMLButtonElement).click();
    await harness.fixture.whenStable();
  };
  const reply = async (url: string, body: object | null, status = 200): Promise<TestRequest> => {
    const request = http.expectOne(url);
    request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error' });
    // A page shown next may load the tenant: answer it, so the app settles.
    for (let i = 0; i < 5; i++) {
      await new Promise((resolve) => setTimeout(resolve));
      for (const tenant of http.match('/api/v1/tenant')) {
        tenant.flush(null, { status: 404, statusText: 'Not Found' });
      }
      for (const providers of http.match('/api/v1/auth/providers')) {
        providers.flush([]);
      }
    }
    await harness.fixture.whenStable();
    return request;
  };
  const text = (selector: string) => element.querySelector(selector)?.textContent?.replace(/\s+/g, ' ').trim();
  return { harness, http, element, fill, submit, reply, text };
}

afterEach(() => TestBed.inject(HttpTestingController).verify());

describe('SignInPage', () => {
  async function openSignIn(url = '/sign-in', providers: object[] = []) {
    const page = await open(url);
    page.http.expectOne('/api/v1/tenant').flush(null, { status: 404, statusText: 'Not Found' });
    page.http.expectOne('/api/v1/auth/providers').flush(providers);
    await page.harness.fixture.whenStable();
    return page;
  }

  it('checks the fields before calling the API', async () => {
    const { submit, element, http } = await openSignIn();
    await submit();
    http.expectNone('/api/v1/auth/sign-in');
    const errors = Array.from(element.querySelectorAll('mat-error')).map((e) => e.textContent?.trim());
    expect(errors).toEqual(['This field is required.', 'This field is required.']);
    expect(await axeViolations(element)).toEqual([]);
  });

  it('says why sign-in failed, and clears the password', async () => {
    const { fill, submit, reply, text, element } = await openSignIn();
    fill('email', 'lucas.bernard@acme.example');
    fill('password', 'wrong password');
    await submit();
    await reply('/api/v1/auth/sign-in', problem(401, 'auth.invalid_credentials'), 401);
    expect(text('[role="alert"]')).toBe('Incorrect email or password.');
    expect((element.querySelector('input[formcontrolname="password"]') as HTMLInputElement).value).toBe('');

    fill('password', 'another try!');
    await submit();
    await reply('/api/v1/auth/sign-in', problem(429, 'too_many_requests'), 429);
    expect(text('[role="alert"]')).toBe('Too many attempts. Try again in a few minutes.');
  });

  it('never follows a return URL to another site', async () => {
    const { fill, submit, reply } = await openSignIn('/sign-in?returnUrl=%2F%2Fevil.example');
    fill('email', 'lucas.bernard@acme.example');
    fill('password', 'correct horse battery');
    await submit();
    await reply('/api/v1/auth/sign-in', SESSION);
    expect(TestBed.inject(Router).url).toBe('/');
  });

  it('offers the tenant\'s identity providers, keeping the way back', async () => {
    const { element, text } = await openSignIn('/sign-in?returnUrl=%2Fgantt', [
      { id: 'p1', displayName: 'Entra ID' },
    ]);
    const button = element.querySelector('a.provider') as HTMLAnchorElement;
    expect(button.textContent?.trim()).toContain('Continue with Entra ID');
    expect(button.getAttribute('href')).toBe('/api/v1/auth/oidc/p1/start?returnUrl=%2Fgantt');
    expect(text('.divider')).toBe('or with a local account');
    expect(await axeViolations(element)).toEqual([]);
  });

  it('explains why a sign-in through the provider failed', async () => {
    const { text } = await openSignIn('/sign-in?error=auth.oidc_no_member');
    expect(text('[role="alert"]')).toBe('No member of this organization has the email of this account.');
  });

  it('ignores an error code it does not know', async () => {
    const { text } = await openSignIn('/sign-in?error=%3Cscript%3E');
    expect(text('[role="alert"]')).toBe('An unexpected error occurred.');
  });

  it('points to the provider when the organization requires it', async () => {
    const { fill, submit, reply, text } = await openSignIn('/sign-in', [{ id: 'p1', displayName: 'Entra ID' }]);
    fill('email', 'lucas.bernard@acme.example');
    fill('password', 'correct horse battery');
    await submit();
    await reply('/api/v1/auth/sign-in', problem(403, 'auth.sso_required'), 403);
    expect(text('[role="alert"]')).toBe('Your organization signs you in through its identity provider: use the button above.');
  });

  it('confirms that the account was erased', async () => {
    const { text } = await openSignIn('/sign-in?accountErased=true');
    expect(text('[role="status"]')).toBe('Your account was deleted.');
  });

  it('confirms a password change and links to the reset request', async () => {
    const { text, element } = await openSignIn('/sign-in?passwordChanged=1');
    expect(text('[role="status"]')).toBe('Your password has been changed. Sign in with the new one.');
    expect(element.querySelector('a[href="/forgot-password"]')?.textContent?.trim()).toBe('Forgot your password?');
  });
});

describe('ForgotPasswordPage', () => {
  it('confirms the request without telling whether the account exists', async () => {
    const { fill, submit, reply, text, element } = await open('/forgot-password');
    expect(element.querySelector('h1')?.textContent).toBe('Forgot your password');
    fill('email', 'not-an-email');
    await submit();
    expect(text('mat-error')).toBe('Enter a valid email address.');

    fill('email', 'someone@acme.example');
    await submit();
    const request = await reply('/api/v1/auth/password-reset', null, 202);
    expect(request.request.body).toEqual({ email: 'someone@acme.example' });
    expect(text('[role="status"]')).toBe(
      'If an account matches someone@acme.example, an email with a link is on its way. The link is valid for one hour.',
    );
    expect(element.querySelector('form')).toBeNull();
    expect(element.querySelector('a[href="/sign-in"]')).not.toBeNull();
    expect(await axeViolations(element)).toEqual([]);
  });

  it('shows a failure, such as too many requests', async () => {
    const { fill, submit, reply, text } = await open('/forgot-password');
    fill('email', 'someone@acme.example');
    await submit();
    await reply('/api/v1/auth/password-reset', problem(429, 'too_many_requests'), 429);
    expect(text('[role="alert"]')).toBe('Too many attempts. Try again in a few minutes.');
  });
});

describe('ResetPasswordPage', () => {
  it('explains a link without token', async () => {
    const { text, element } = await open('/reset-password');
    expect(text('[role="alert"]')).toBe('This link is incomplete. Ask for a new one.');
    expect(element.querySelector('a[href="/forgot-password"]')).not.toBeNull();
    expect(element.querySelector('form')).toBeNull();
  });

  it('checks the length and the confirmation before calling the API', async () => {
    const { fill, submit, http, element } = await open('/reset-password?token=abc');
    fill('password', 'short');
    fill('confirmation', 'shorter');
    await submit();
    http.expectNone('/api/v1/auth/password-reset/confirm');
    const errors = Array.from(element.querySelectorAll('mat-error')).map((e) => e.textContent?.trim());
    expect(errors).toEqual(['This is too short.', 'The two passwords differ.']);
    expect(await axeViolations(element)).toEqual([]);
  });

  it('sets the new password, then shows the sign-in page', async () => {
    const { fill, submit, reply } = await open('/reset-password?token=abc');
    fill('password', 'a brand new passphrase');
    fill('confirmation', 'a brand new passphrase');
    await submit();
    const request = await reply('/api/v1/auth/password-reset/confirm', null, 204);
    expect(request.request.body).toEqual({ token: 'abc', password: 'a brand new passphrase' });
    expect(TestBed.inject(Router).url).toBe('/sign-in?passwordChanged=1');
  });

  it('shows the field errors of the API and offers a new link when the token is refused', async () => {
    const { fill, submit, reply, text, element } = await open('/reset-password?token=abc');
    fill('password', 'lucas@acme.example');
    fill('confirmation', 'lucas@acme.example');
    await submit();
    await reply(
      '/api/v1/auth/password-reset/confirm',
      problem(400, 'validation_failed', [{ field: 'password', code: 'same_as_email' }]),
      400,
    );
    expect(text('mat-error')).toBe('The password must differ from the email.');

    fill('password', 'a brand new passphrase');
    fill('confirmation', 'a brand new passphrase');
    await submit();
    await reply('/api/v1/auth/password-reset/confirm', problem(400, 'auth.reset_token_invalid'), 400);
    expect(text('[role="alert"]')).toContain('This link is invalid, has expired or was already used.');
    expect(element.querySelector('[role="alert"] a[href="/forgot-password"]')).not.toBeNull();
  });
});
