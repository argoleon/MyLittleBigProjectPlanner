/**
 * Integration tests of the "Authentication" tab of the administration, with
 * the API mocked at the HTTP level and the real translations.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import en from '../../../../ui/i18n/en.json';
import fr from '../../../../ui/i18n/fr.json';
import { AdminPage } from './admin-page';
import { AuthenticationSettings, POLICY_TOGGLES } from './authentication-settings';

const POLICY = {
  internalAccountsRequireSso: false,
  localAccountsRequireMfa: true,
  portalPasswordEnabled: true,
  portalMagicLinkEnabled: true,
};

const PROVIDER = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a8c',
  displayName: 'Entra ID',
  issuer: 'https://login.microsoftonline.com/acme/v2.0',
  clientId: 'client-1',
  emailClaim: 'email',
  enabled: true,
  redirectUri: 'https://acme.example.org/api/v1/auth/oidc/callback',
  version: 3,
  createdAt: '2026-11-10T09:00:00Z',
  updatedAt: '2026-11-10T09:00:00Z',
};

function problem(status: number, code: string, detail?: string) {
  return { type: `urn:mlbpp:problem:${code}`, title: code, status, code, detail };
}

async function open({ policy = POLICY as object, providers = [PROVIDER] as object[] } = {}) {
  localStorage.clear();
  TestBed.configureTestingModule({
    providers: [
      provideRouter([
        {
          path: 'admin',
          component: AdminPage,
          children: [{ path: 'authentication', component: AuthenticationSettings }],
        },
      ]),
      provideApi(),
      provideHttpClientTesting(),
      provideI18nTesting(),
      provideMaterialDefaults(),
    ],
  });
  const harness = await RouterTestingHarness.create();
  await harness.navigateByUrl('/admin/authentication');
  const http = TestBed.inject(HttpTestingController);
  // Not `whenStable`: pending resources keep the app unstable.
  const settle = async () => {
    for (let i = 0; i < 3; i++) {
      await new Promise((resolve) => setTimeout(resolve));
      TestBed.tick();
    }
  };
  const reply = async (
    url: string,
    body: object | null,
    status = 200,
    headers: Record<string, string> = {},
  ): Promise<TestRequest> => {
    let request: TestRequest | undefined;
    // Up to a second: a dialog closes after its exit animation.
    for (let i = 0; i < 50 && !request; i++) {
      [request] = http.match(url);
      if (!request) await new Promise((resolve) => setTimeout(resolve, 20));
    }
    if (!request) throw new Error(`no request to ${url}`);
    request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error', headers });
    await settle();
    return request;
  };
  await reply('/api/v1/tenant', { id: 't', slug: 'acme', name: 'Acme Industrie', defaultLocale: 'fr', timeZone: 'Europe/Paris' });
  await reply('/api/v1/tenant/auth-policy', policy, policy === POLICY ? 200 : 403, { ETag: '"4"' });
  await reply('/api/v1/tenant/oidc-providers', providers);
  const root = () => document.body;
  const text = (selector: string) => root().querySelector(selector)?.textContent?.replace(/\s+/g, ' ').trim();
  const click = async (element: Element | null) => {
    (element as HTMLElement).click();
    await settle();
  };
  return { harness, http, reply, settle, root, text, click };
}

describe('AuthenticationSettings', () => {
  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify();
  });

  it('shows the policy and the providers of the tenant', async () => {
    const { root, text } = await open();
    expect(text('.eyebrow')).toBe('Administration · Acme Industrie');
    expect(text('a[mat-tab-link]')).toBe('Authentication');
    const toggles = Array.from(root().querySelectorAll('mat-slide-toggle'));
    expect(toggles.map((t) => t.textContent?.trim())).toEqual([
      'Internal accounts sign in through the identity provider',
      'Local accounts need two-step verification',
      'Portal: sign-in with a password',
      'Portal: sign-in with a link sent by email',
    ]);
    const checked = toggles.map((t) => t.querySelector('button')?.getAttribute('aria-checked'));
    expect(checked).toEqual(['false', 'true', 'true', 'true']);
    expect(text('.provider-name')).toBe('Entra ID');
    expect(text('.provider-issuer')).toBe(PROVIDER.issuer);
    expect(text('.status')).toBe('Enabled');
    expect(await axeViolations(root().querySelector('app-admin-page')!)).toEqual([]);
  });

  it('saves the policy with the version it was read at', async () => {
    const { root, reply, text, click } = await open();
    await click(root().querySelector('mat-slide-toggle button'));
    await click(root().querySelector('form button[type="submit"]'));
    const put = await reply('/api/v1/tenant/auth-policy', { ...POLICY, internalAccountsRequireSso: true }, 200, {
      ETag: '"5"',
    });
    expect(put.request.method).toBe('PUT');
    expect(put.request.headers.get('If-Match')).toBe('"4"');
    expect(put.request.body).toEqual({ ...POLICY, internalAccountsRequireSso: true });
    expect(text('[role="status"]')).toBe('The policy is saved.');
    // The next save uses the new version.
    await click(root().querySelector('mat-slide-toggle button'));
    await click(root().querySelector('form button[type="submit"]'));
    const next = await reply('/api/v1/tenant/auth-policy', POLICY, 200, { ETag: '"6"' });
    expect(next.request.headers.get('If-Match')).toBe('"5"');
  });

  it('warns that SSO cannot be required without an enabled provider', async () => {
    const { root, text, click } = await open({ providers: [{ ...PROVIDER, enabled: false }] });
    expect(text('.status')).toBe('Disabled');
    expect(root().querySelector('.message.warning')).toBeNull();
    await click(root().querySelector('mat-slide-toggle button'));
    expect(text('.message.warning')).toBe('No identity provider is enabled: the rule applies once one is.');
  });

  it('tests and deletes a provider', async () => {
    const { root, reply, text, click } = await open();
    await click(root().querySelector('.provider-actions button'));
    await reply(`/api/v1/tenant/oidc-providers/${PROVIDER.id}/test`, problem(502, 'oidc_provider.unreachable', 'HTTP 404'), 502);
    expect(text('.provider [role="alert"]')).toBe('The identity provider cannot be reached or answered badly. HTTP 404');
    await click(root().querySelector('.provider-actions button'));
    await reply(`/api/v1/tenant/oidc-providers/${PROVIDER.id}/test`, {
      issuer: PROVIDER.issuer,
      authorizationEndpoint: 'https://x/auth',
      tokenEndpoint: 'https://x/token',
    });
    expect(text('.provider [role="status"]')).toBe('The provider answers.');

    await click(root().querySelector('button[aria-label="Delete Entra ID"]'));
    const deleted = await reply(`/api/v1/tenant/oidc-providers/${PROVIDER.id}`, null, 204);
    expect(deleted.request.method).toBe('DELETE');
    expect(deleted.request.headers.get('If-Match')).toBe('"3"');
    await reply('/api/v1/tenant/oidc-providers', []);
    expect(text('.empty')).toBe('No identity provider yet: members sign in with their password.');
  });

  it('adds a provider through the dialog, its secret required', async () => {
    const { root, reply, click, settle } = await open({ providers: [] });
    await click(root().querySelector('.card-header button'));
    const dialog = () => root().querySelector('mat-dialog-container')!;
    expect(dialog().querySelector('h2')?.textContent).toBe('Add a provider');
    await click(dialog().querySelector('button[type="submit"]'));
    expect(dialog().querySelectorAll('mat-error').length).toBe(4);

    const fill = (name: string, value: string) => {
      const input = dialog().querySelector(`input[formcontrolname="${name}"]`) as HTMLInputElement;
      input.value = value;
      input.dispatchEvent(new Event('input'));
    };
    fill('displayName', 'Entra ID');
    fill('issuer', 'not a url');
    fill('clientId', 'client-1');
    fill('clientSecret', 's3cret');
    await settle();
    await click(dialog().querySelector('button[type="submit"]'));
    expect(dialog().querySelector('mat-error')?.textContent?.trim()).toBe('The format is invalid.');
    fill('issuer', PROVIDER.issuer);
    await click(dialog().querySelector('button[type="submit"]'));
    const created = await reply('/api/v1/tenant/oidc-providers', PROVIDER, 201);
    expect(created.request.method).toBe('POST');
    expect(created.request.body).toEqual({
      displayName: 'Entra ID',
      issuer: PROVIDER.issuer,
      clientId: 'client-1',
      clientSecret: 's3cret',
      emailClaim: 'email',
      enabled: true,
    });
    await reply('/api/v1/tenant/oidc-providers', [PROVIDER]);
    expect(root().querySelector('mat-dialog-container')).toBeNull();
  });

  it('edits a provider without sending its secret back', async () => {
    const { root, reply, click } = await open();
    await click(root().querySelector('button[aria-label="Edit Entra ID"]'));
    const dialog = root().querySelector('mat-dialog-container')!;
    expect(dialog.querySelector('code')?.textContent).toBe(PROVIDER.redirectUri);
    expect((dialog.querySelector('input[formcontrolname="clientSecret"]') as HTMLInputElement).value).toBe('');
    await click(dialog.querySelector('button[type="submit"]'));
    const updated = await reply(`/api/v1/tenant/oidc-providers/${PROVIDER.id}`, PROVIDER);
    expect(updated.request.method).toBe('PUT');
    expect(updated.request.headers.get('If-Match')).toBe('"3"');
    expect(updated.request.body).not.toHaveProperty('clientSecret');
    await reply('/api/v1/tenant/oidc-providers', [PROVIDER]);
  });

  it('shows the refusal of a member who is not a tenant admin', async () => {
    const { text } = await open({ policy: problem(403, 'forbidden') });
    expect(text('ui-error-state p')).toBe('You are not allowed to do this.');
  });
});

describe('policy translations', () => {
  it('have a label and a hint for every toggle', () => {
    for (const [lang, file] of [['fr', fr], ['en', en]] as const) {
      const policy = file.admin.auth.policy as Record<string, { label?: string; hint?: string }>;
      for (const toggle of POLICY_TOGGLES) {
        expect(policy[toggle]?.label, `${lang} ${toggle}`).toBeTruthy();
        expect(policy[toggle]?.hint, `${lang} ${toggle}`).toBeTruthy();
      }
    }
  });
});
