/**
 * Integration test of the request portal: boots the root component with the
 * portal's routes and providers.
 */
import { provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { Router, provideRouter, withComponentInputBinding } from '@angular/router';
import { LocaleService, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { App } from './app';
import { routes } from './app.routes';

async function boot() {
  localStorage.clear();
  TestBed.configureTestingModule({
    providers: [
      provideRouter(routes, withComponentInputBinding()),
      provideApi(),
      provideHttpClientTesting(),
      provideI18nTesting({ appNameKey: 'app.portalName' }),
      provideMaterialDefaults(),
    ],
  });
  const fixture = TestBed.createComponent(App);
  await TestBed.inject(Router).navigateByUrl('/');
  await fixture.whenStable();
  return fixture;
}

describe('Portal', () => {
  it('renders the portal shell and its home page', async () => {
    const fixture = await boot();
    const element = fixture.nativeElement as HTMLElement;
    expect(element.querySelector('header .brand-name')?.textContent).toBe('Request portal');
    expect(element.querySelector('main h1')?.textContent).toBe('Welcome to the portal');
    expect(document.title).toBe('Welcome to the portal · Request portal');
  });

  it('follows the language of the user', async () => {
    const fixture = await boot();
    TestBed.inject(LocaleService).use('fr');
    await fixture.whenStable();
    expect((fixture.nativeElement as HTMLElement).querySelector('main h1')?.textContent).toBe('Bienvenue sur le portail');
    expect(document.title).toBe('Bienvenue sur le portail · Portail de demandes');
  });

  it('has no accessibility violation', async () => {
    const fixture = await boot();
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
  });
});
