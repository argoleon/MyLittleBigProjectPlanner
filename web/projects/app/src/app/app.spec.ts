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

/**
 * Boots the application on `/`, answering the tenant request with `tenant`
 * (`null`: the API answers 404), then waits for it to settle.
 */
async function boot(tenant: object | null = null): Promise<{ fixture: ComponentFixture<App>; request: TestRequest }> {
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
  await TestBed.inject(Router).navigateByUrl('/');
  TestBed.tick();
  const request = TestBed.inject(HttpTestingController).expectOne('/api/v1/tenant');
  if (tenant) {
    request.flush(tenant);
  } else {
    request.flush(null, { status: 404, statusText: 'Not Found' });
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
});
