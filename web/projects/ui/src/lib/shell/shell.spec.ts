import { BreakpointObserver } from '@angular/cdk/layout';
import { TestbedHarnessEnvironment } from '@angular/cdk/testing/testbed';
import { Component } from '@angular/core';
import { ComponentFixture, TestBed } from '@angular/core/testing';
import { MatMenuHarness } from '@angular/material/menu/testing';
import { MatSidenavHarness } from '@angular/material/sidenav/testing';
import { Router, RouterOutlet, provideRouter } from '@angular/router';
import { of } from 'rxjs';

import { axeViolations } from '../testing/axe';
import { provideI18nTesting } from '../testing/i18n-testing';
import { NavItem } from './nav-item';
import { Shell } from './shell';

@Component({ selector: 'test-home', template: '<h1>Home page</h1>' })
class HomePage {}

@Component({ selector: 'test-workload', template: '<h1>Workload page</h1>' })
class WorkloadPage {}

const NAV: NavItem[] = [
  { route: '/', labelKey: 'nav.home', icon: 'home', exact: true },
  { route: '/workload', labelKey: 'nav.workload', icon: 'groups' },
];

@Component({
  imports: [Shell, RouterOutlet],
  template: `<ui-shell [navItems]="nav"><p uiShellDrawerHeader id="drawer-header">Tenant</p><router-outlet /></ui-shell>`,
})
class Host {
  readonly nav = NAV;
}

async function setUp(modal = false): Promise<ComponentFixture<Host>> {
  localStorage.clear();
  TestBed.configureTestingModule({
    providers: [
      provideRouter([
        { path: '', component: HomePage, title: 'nav.home' },
        { path: 'workload', component: WorkloadPage, title: 'nav.workload' },
      ]),
      provideI18nTesting(),
      { provide: BreakpointObserver, useValue: { observe: () => of({ matches: modal, breakpoints: {} }) } },
    ],
  });
  const fixture = TestBed.createComponent(Host);
  await TestBed.inject(Router).navigateByUrl('/');
  await fixture.whenStable();
  return fixture;
}

function links(fixture: ComponentFixture<Host>): HTMLAnchorElement[] {
  return Array.from((fixture.nativeElement as HTMLElement).querySelectorAll<HTMLAnchorElement>('nav a'));
}

async function navigate(fixture: ComponentFixture<Host>, url: string): Promise<void> {
  await TestBed.inject(Router).navigateByUrl(url);
  await fixture.whenStable();
}

describe('Shell', () => {
  it('renders the navigation with translated labels, the projected header and the page', async () => {
    const fixture = await setUp();
    const element = fixture.nativeElement as HTMLElement;
    expect(links(fixture).map((a) => a.textContent?.trim())).toEqual(['homeHome', 'groupsWorkload']);
    expect(element.querySelector('nav')?.getAttribute('aria-label')).toBe('Main navigation');
    expect(element.querySelector('.drawer #drawer-header')).not.toBeNull();
    expect(element.querySelector('main h1')?.textContent).toBe('Home page');
    expect(element.querySelector('.brand-name')?.textContent).toBe('My Little Big Project Planner');
  });

  it('highlights the active entry and moves it on navigation', async () => {
    const fixture = await setUp();
    let [home, workload] = links(fixture);
    expect(home.getAttribute('aria-current')).toBe('page');
    expect(home.classList).toContain('mdc-list-item--activated');
    expect(workload.hasAttribute('aria-current')).toBe(false);

    workload.click();
    await fixture.whenStable();
    [home, workload] = links(fixture);
    expect(TestBed.inject(Router).url).toBe('/workload');
    expect(workload.getAttribute('aria-current')).toBe('page');
    expect(workload.classList).toContain('mdc-list-item--activated');
    // The home entry matches its exact URL only.
    expect(home.hasAttribute('aria-current')).toBe(false);
    expect((fixture.nativeElement as HTMLElement).querySelector('main h1')?.textContent).toBe('Workload page');
  });

  it('sets a translated page title', async () => {
    const fixture = await setUp();
    await navigate(fixture, '/workload');
    expect(document.title).toBe('Workload · My Little Big Project Planner');
  });

  it('moves the focus to the main area with the skip link, without navigating', async () => {
    const fixture = await setUp();
    await navigate(fixture, '/workload');
    const skip = (fixture.nativeElement as HTMLElement).querySelector<HTMLAnchorElement>('.skip-link')!;
    expect(skip.textContent).toBe('Skip to content');
    skip.click();
    expect(document.activeElement?.id).toBe('main');
    expect(TestBed.inject(Router).url).toBe('/workload');
  });

  it('keeps the drawer open on wide screens, without a menu button', async () => {
    const fixture = await setUp(false);
    const drawer = await TestbedHarnessEnvironment.loader(fixture).getHarness(MatSidenavHarness);
    expect(await drawer.isOpen()).toBe(true);
    expect(await drawer.getMode()).toBe('side');
    expect((fixture.nativeElement as HTMLElement).querySelector('[aria-label="Open navigation"]')).toBeNull();
  });

  it('uses a modal drawer on narrow screens, closed after navigation', async () => {
    const fixture = await setUp(true);
    const drawer = await TestbedHarnessEnvironment.loader(fixture).getHarness(MatSidenavHarness);
    expect(await drawer.isOpen()).toBe(false);
    expect(await drawer.getMode()).toBe('over');

    (fixture.nativeElement as HTMLElement).querySelector<HTMLButtonElement>('[aria-label="Open navigation"]')!.click();
    await fixture.whenStable();
    expect(await drawer.isOpen()).toBe(true);

    links(fixture)[1].click();
    await fixture.whenStable();
    expect(await drawer.isOpen()).toBe(false);
  });

  it('switches every text to French from the language menu', async () => {
    const fixture = await setUp();
    await navigate(fixture, '/workload');
    const loader = TestbedHarnessEnvironment.loader(fixture);
    const [languageMenu] = await loader.getAllHarnesses(MatMenuHarness);
    expect(await (await languageMenu.host()).getAttribute('aria-label')).toBe('Language: English');

    await languageMenu.clickItem({ text: /Français/ });
    await fixture.whenStable();

    expect(links(fixture).map((a) => a.textContent?.trim())).toEqual(['homeAccueil', 'groupsCharge']);
    const element = fixture.nativeElement as HTMLElement;
    expect(element.querySelector('nav')?.getAttribute('aria-label')).toBe('Navigation principale');
    expect(element.querySelector('.skip-link')?.textContent).toBe('Aller au contenu');
    expect(await (await languageMenu.host()).getAttribute('aria-label')).toBe('Langue : Français');
    expect(document.title).toBe('Charge · My Little Big Project Planner');
    expect(document.documentElement.lang).toBe('fr');
  });

  it('has no accessibility violation', async () => {
    const fixture = await setUp();
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
  });
});
