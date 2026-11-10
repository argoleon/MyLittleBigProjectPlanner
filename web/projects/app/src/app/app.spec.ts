/**
 * Integration test of the internal application: boots the root component
 * with the application's routes and providers (translations loaded
 * synchronously, API mocked at the HTTP level).
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { ComponentFixture, TestBed } from '@angular/core/testing';
import { Router, provideRouter, withComponentInputBinding } from '@angular/router';
import { provideApi, provideMaterialDefaults } from 'ui';
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

/** The session `GET /api/v1/auth/session` returns when signed in. */
const SESSION = {
  account: {
    id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a8c',
    email: 'lucas.bernard@acme.example',
    displayName: 'Lucas Bernard',
    locale: null,
    tenantRole: 'member',
  },
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

/**
 * Boots the application on `url`, signed in (`session`) or not (`null`),
 * answering the tenant request with `tenant` (`null`: the API answers 404),
 * then waits for it to settle.
 */
async function boot(
  tenant: object | null = null,
  { url = '/', session = SESSION as object | null } = {},
): Promise<{ fixture: ComponentFixture<App>; request: TestRequest }> {
  localStorage.clear();
  TestBed.configureTestingModule({
    providers: [
      provideRouter(routes, withComponentInputBinding()),
      provideApi(),
      provideHttpClientTesting(),
      provideI18nTesting(),
      provideMaterialDefaults(),
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
  const request = await answer('/api/v1/tenant', tenant, tenant ? 200 : 404);
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
    await answer('/api/v1/tenant', TENANT);
    await fixture.whenStable();

    expect(router.url).toBe('/workload');
    expect(query(fixture, 'main h1')?.textContent).toBe('Workload');
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
    await fixture.whenStable();
    expect(TestBed.inject(Router).url).toBe('/sign-in');
  });
});
