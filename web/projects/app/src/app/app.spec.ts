/**
 * Integration test of the internal application: boots the root component
 * with the application's routes and providers (translations loaded
 * synchronously, API mocked at the HTTP level).
 */
import { TestbedHarnessEnvironment } from '@angular/cdk/testing/testbed';
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { Provider } from '@angular/core';
import { ComponentFixture, TestBed } from '@angular/core/testing';
import { MatSelectHarness } from '@angular/material/select/testing';
import { Router, provideRouter, withComponentInputBinding } from '@angular/router';
import { AuthService, LEAVE_APPLICATION, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { App } from './app';
import { routes } from './app.routes';
import { NAVIGATION } from './navigation';

const TENANT = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a8b',
  slug: 'acme',
  name: 'Acme Industrie',
  defaultLocale: 'fr',
  timeZone: 'Europe/Paris',
};

const MEMBERSHIPS_URL = '/api/v1/me/memberships';

/** The member's tenant in `GET /api/v1/me/memberships`. */
const ACME = {
  tenantId: TENANT.id,
  slug: 'acme',
  name: 'Acme Industrie',
  url: 'https://acme.planner.example',
  accountType: 'internal',
  tenantRole: 'member',
  current: true,
};

/** Another tenant of the member. */
const GLOBEX = {
  tenantId: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a90',
  slug: 'globex',
  name: 'Globex',
  url: 'https://globex.planner.example',
  accountType: 'internal',
  tenantRole: 'tenant_admin',
  current: false,
};

/** The session `GET /api/v1/auth/session` returns when signed in. */
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

const UNAUTHENTICATED = {
  type: 'urn:mlbpp:problem:unauthenticated',
  title: 'Sign-in required, or the session has ended',
  status: 401,
  code: 'unauthenticated',
};

const http = () => TestBed.inject(HttpTestingController);

/**
 * Answers the request to `url` with `body`, or a Problem, once it is sent
 * (route guards and resources send theirs asynchronously).
 */
async function answer(url: string, body: object | null, status = 200): Promise<TestRequest> {
  let request: TestRequest | undefined;
  for (let i = 0; i < 20 && !request; i++) {
    [request] = http().match(url);
    if (!request) await new Promise((resolve) => setTimeout(resolve));
  }
  if (!request) throw new Error(`no request to ${url}`);
  if (status < 400) {
    request.flush(body, { status, statusText: 'OK' });
  } else {
    request.flush(body, { status, statusText: 'Error', headers: { 'content-type': 'application/problem+json' } });
  }
  return request;
}

/** `GET /api/v1/me`: the signed-in account, without preferences of its own. */
const ME = {
  id: SESSION.account.id,
  email: SESSION.account.email,
  displayName: SESSION.account.displayName,
  locale: null,
  timeZone: null,
  hasPassword: true,
  tenantId: TENANT.id,
  tenantRole: 'member',
  accountType: 'internal',
};

/**
 * Answers what the frame of signed-in members asks once shown: the
 * account's tenants (`memberships`), the account and the tenant, whose
 * preferences apply (`tenant` null: the API answers 404).
 */
async function answerFrame(memberships: object[], tenant: object | null = TENANT): Promise<TestRequest> {
  const request = await answer(MEMBERSHIPS_URL, memberships);
  await answer('/api/v1/me', ME);
  await answer('/api/v1/tenant', tenant, tenant ? 200 : 404);
  return request;
}

/**
 * Boots the application on `url`, signed in (`session`) or not (`null`),
 * then waits for it to settle. Signed in, the drawer lists `memberships`
 * (by default `tenant`'s, none without tenant); signed out, the sign-in page
 * shows `tenant` (`null`: the API answers 404).
 */
async function boot(
  tenant: object | null = null,
  {
    url = '/',
    session = SESSION as object | null,
    memberships = undefined as object[] | undefined,
    providers = [] as Provider[],
  } = {},
): Promise<{ fixture: ComponentFixture<App>; request: TestRequest }> {
  localStorage.clear();
  TestBed.configureTestingModule({
    providers: [
      provideRouter(routes, withComponentInputBinding()),
      provideApi(),
      provideHttpClientTesting(),
      provideI18nTesting(),
      provideMaterialDefaults(),
      ...providers,
    ],
  });
  const fixture = TestBed.createComponent(App);
  const navigated = TestBed.inject(Router).navigateByUrl(url);
  // The route guard asks for the session first.
  if (session) {
    await answer('/api/v1/auth/session', session);
  } else {
    await answer('/api/v1/auth/session', UNAUTHENTICATED, 401);
  }
  await navigated;
  TestBed.tick();
  let request: TestRequest;
  if (session) {
    request = await answerFrame(memberships ?? (tenant ? [ACME] : []), tenant);
  } else {
    request = await answer('/api/v1/tenant', tenant, tenant ? 200 : 404);
    // The sign-in page also lists the tenant's identity providers.
    await answer('/api/v1/auth/providers', []);
  }
  await fixture.whenStable();
  return { fixture, request };
}

async function navigate(fixture: ComponentFixture<App>, url: string): Promise<void> {
  await TestBed.inject(Router).navigateByUrl(url);
  await fixture.whenStable();
}

const query = (fixture: ComponentFixture<App>, selector: string) =>
  (fixture.nativeElement as HTMLElement).querySelector(selector);

describe('App', () => {
  afterEach(() => TestBed.inject(HttpTestingController).verify());

  it('renders the shell with every entry of the navigation', async () => {
    const { fixture } = await boot();
    const labels = Array.from((fixture.nativeElement as HTMLElement).querySelectorAll('nav a [matListItemTitle]')).map(
      (e) => e.textContent,
    );
    expect(labels).toHaveLength(NAVIGATION.length);
    expect(labels).toEqual([
      'Home',
      'My tickets',
      'Search',
      'Boards',
      'Gantt',
      'Workload',
      'Portfolios',
      'Portals',
      'Dashboards',
      'Administration',
    ]);
  });

  it('shows the current tenant loaded through the API client', async () => {
    const { fixture, request } = await boot(TENANT);
    expect(request.request.withCredentials).toBe(true);
    expect(query(fixture, 'ui-tenant-switcher .mat-mdc-select-value')?.textContent?.trim()).toBe('Acme Industrie');
    const select = await TestbedHarnessEnvironment.loader(fixture).getHarness(MatSelectHarness);
    await select.open();
    expect(await Promise.all((await select.getOptions()).map((o) => o.getText()))).toEqual(['Acme IndustrieMember']);
  });

  it('lists every tenant of the member, with the role in each, and the current one selected', async () => {
    const { fixture } = await boot(TENANT, { memberships: [ACME, GLOBEX] });
    expect(query(fixture, 'ui-tenant-switcher .mat-mdc-select-value')?.textContent?.trim()).toBe('Acme Industrie');
    const select = await TestbedHarnessEnvironment.loader(fixture).getHarness(MatSelectHarness);
    await select.open();
    const options = await select.getOptions();
    expect(await Promise.all(options.map((o) => o.getText()))).toEqual([
      'Acme IndustrieMember',
      'GlobexOrganization admin',
    ]);
    expect(await options[0].isSelected()).toBe(true);
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
    expect(await axeViolations(document.querySelector('.cdk-overlay-container') as HTMLElement)).toEqual([]);
  });

  it('switches to another tenant, leaving nothing of this one behind', async () => {
    const leave = vi.fn<(url: string) => void>();
    const { fixture } = await boot(TENANT, {
      memberships: [ACME, GLOBEX],
      providers: [{ provide: LEAVE_APPLICATION, useValue: leave }],
    });
    const select = await TestbedHarnessEnvironment.loader(fixture).getHarness(MatSelectHarness);
    await select.clickOptions({ text: /^Globex/ });
    const request = await answer('/api/v1/me/switch', { url: 'https://globex.planner.example/api/v1/auth/switch?token=t' });
    expect(request.request.method).toBe('POST');
    expect(request.request.body).toEqual({ tenantId: GLOBEX.tenantId });
    await fixture.whenStable();
    expect(leave).toHaveBeenCalledExactlyOnceWith('https://globex.planner.example/api/v1/auth/switch?token=t');
    expect(TestBed.inject(AuthService).session()).toBeNull();
  });

  it('stays on the current tenant with a message when the switch is refused', async () => {
    const leave = vi.fn<(url: string) => void>();
    const { fixture } = await boot(TENANT, {
      memberships: [ACME, GLOBEX],
      providers: [{ provide: LEAVE_APPLICATION, useValue: leave }],
    });
    const select = await TestbedHarnessEnvironment.loader(fixture).getHarness(MatSelectHarness);
    await select.clickOptions({ text: /^Globex/ });
    await answer('/api/v1/me/switch', { code: 'forbidden', title: 'You are not allowed to do this', status: 403 }, 403);
    await new Promise((resolve) => setTimeout(resolve));
    await fixture.whenStable();
    expect(leave).not.toHaveBeenCalled();
    expect(document.querySelector('.mat-mdc-snack-bar-label')?.textContent?.trim()).toBe('You are not allowed to do this.');
    expect(await select.getValueText()).toBe('Acme Industrie');
    expect(TestBed.inject(AuthService).signedIn()).toBe(true);
  });

  it('navigates between two placeholder screens', async () => {
    const { fixture } = await boot();
    expect(query(fixture, 'main h1')?.textContent).toBe('Home');
    expect(document.title).toBe('Home · My Little Big Project Planner');

    query(fixture, 'nav a[href="/workload"]')?.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
    await fixture.whenStable();
    expect(TestBed.inject(Router).url).toBe('/workload');
    expect(query(fixture, 'main h1')?.textContent).toBe('Workload');
    expect(query(fixture, 'main [role="heading"]')?.textContent).toBe('Coming soon');
    expect(query(fixture, 'nav a[aria-current="page"]')?.getAttribute('href')).toBe('/workload');
    expect(document.title).toBe('Workload · My Little Big Project Planner');

    await navigate(fixture, '/gantt');
    expect(query(fixture, 'main h1')?.textContent).toBe('Gantt');
    expect(query(fixture, 'nav a[aria-current="page"]')?.getAttribute('href')).toBe('/gantt');
  });

  it('redirects unknown URLs to the home screen', async () => {
    const { fixture } = await boot();
    await navigate(fixture, '/does-not-exist');
    expect(TestBed.inject(Router).url).toBe('/');
  });

  it('has no accessibility violation', async () => {
    const { fixture } = await boot();
    await fixture.whenStable();
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
  });

  it('sends signed-out visitors to the sign-in page, then back where they were going', async () => {
    const { fixture } = await boot(TENANT, { url: '/workload', session: null });
    const router = TestBed.inject(Router);
    expect(router.url).toBe('/sign-in?returnUrl=%2Fworkload');
    expect(query(fixture, 'main h1')?.textContent).toBe('Sign in');
    expect(query(fixture, '.subtitle')?.textContent?.trim()).toBe('Organization: Acme Industrie');
    expect(query(fixture, 'ui-shell')).toBeNull();
    expect(document.title).toBe('Sign in · My Little Big Project Planner');

    const input = (name: string) => query(fixture, `input[formcontrolname="${name}"]`) as HTMLInputElement;
    input('email').value = 'lucas.bernard@acme.example';
    input('email').dispatchEvent(new Event('input'));
    input('password').value = 'correct horse battery';
    input('password').dispatchEvent(new Event('input'));
    (query(fixture, 'button[type="submit"]') as HTMLButtonElement).click();
    const signIn = await answer('/api/v1/auth/sign-in', SESSION);
    expect(signIn.request.body).toEqual({ email: 'lucas.bernard@acme.example', password: 'correct horse battery' });
    await answerFrame([ACME]);
    await fixture.whenStable();

    expect(router.url).toBe('/workload');
    expect(query(fixture, 'main h1')?.textContent).toBe('Workload');
  });

  it('asks for the second factor after the password, then goes where the member was going', async () => {
    const { fixture } = await boot(TENANT, { url: '/workload', session: null });
    const router = TestBed.inject(Router);
    const input = (name: string) => query(fixture, `input[formcontrolname="${name}"]`) as HTMLInputElement;
    input('email').value = 'lucas.bernard@acme.example';
    input('email').dispatchEvent(new Event('input'));
    input('password').value = 'correct horse battery';
    input('password').dispatchEvent(new Event('input'));
    (query(fixture, 'button[type="submit"]') as HTMLButtonElement).click();
    await answer('/api/v1/auth/sign-in', { ...SESSION, stage: 'mfa_challenge' });
    await fixture.whenStable();
    expect(router.url).toBe('/sign-in/verify?returnUrl=%2Fworkload');
    expect(query(fixture, 'main h1')?.textContent).toBe('Two-step verification');

    // The application stays closed meanwhile.
    await router.navigateByUrl('/gantt');
    expect(router.url).toBe('/sign-in/verify?returnUrl=%2Fgantt');

    input('code').value = '482913';
    input('code').dispatchEvent(new Event('input'));
    (query(fixture, 'button[type="submit"]') as HTMLButtonElement).click();
    await answer('/api/v1/auth/sign-in/verify', SESSION);
    await answerFrame([ACME]);
    await fixture.whenStable();
    expect(router.url).toBe('/gantt');
  });

  it('opens My account from the account menu, and keeps the language picked for the account', async () => {
    const { fixture } = await boot(TENANT);
    (query(fixture, 'ui-account-menu button') as HTMLButtonElement).click();
    await fixture.whenStable();
    const myAccount = Array.from(document.querySelectorAll<HTMLButtonElement>('.mat-mdc-menu-panel button')).find((b) =>
      b.textContent?.includes('My account'),
    );
    myAccount!.click();
    // The page is loaded on demand.
    for (let i = 0; i < 100 && TestBed.inject(Router).url !== '/account'; i++) {
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    await answer('/api/v1/account/mfa', { enabled: false, backupCodesLeft: 0, requiredByPolicy: false });
    await answer('/api/v1/me/sessions', []);
    await answer(MEMBERSHIPS_URL, [ACME]);
    await fixture.whenStable();
    expect(TestBed.inject(Router).url).toBe('/account');
    expect(query(fixture, 'main h1')?.textContent).toBe('My account');

    (query(fixture, 'ui-language-menu button') as HTMLButtonElement).click();
    await fixture.whenStable();
    Array.from(document.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]'))
      .find((b) => b.textContent?.includes('Français'))!
      .click();
    const saved = await answer('/api/v1/me/preferences', { locale: 'fr', timeZone: null });
    expect(saved.request.method).toBe('PUT');
    expect(saved.request.body).toEqual({ locale: 'fr', timeZone: null });
    await fixture.whenStable();
    expect(query(fixture, 'main h1')?.textContent).toBe('Mon compte');
  });

  it('shows the signed-in member, and signs out', async () => {
    const { fixture } = await boot(TENANT);
    const account = query(fixture, 'ui-account-menu button') as HTMLButtonElement;
    expect(account.getAttribute('aria-label')).toBe('Account: Lucas Bernard');
    account.click();
    await fixture.whenStable();
    const signOut = Array.from(document.querySelectorAll<HTMLButtonElement>('.mat-mdc-menu-panel button')).find((b) =>
      b.textContent?.includes('Sign out'),
    );
    signOut!.click();
    const request = await answer('/api/v1/auth/sign-out', null, 204);
    expect(request.request.method).toBe('POST');
    await answer('/api/v1/tenant', TENANT);
    await answer('/api/v1/auth/providers', []);
    await fixture.whenStable();
    expect(TestBed.inject(Router).url).toBe('/sign-in');
  });
});
