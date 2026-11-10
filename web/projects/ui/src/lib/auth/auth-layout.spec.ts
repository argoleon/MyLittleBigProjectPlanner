import { Component } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { TestbedHarnessEnvironment } from '@angular/cdk/testing/testbed';
import { MatMenuHarness } from '@angular/material/menu/testing';

import { LocaleService } from '../i18n/locale.service';
import { AccountMenu } from '../shell/account-menu';
import { axeViolations } from '../testing/axe';
import { provideI18nTesting } from '../testing/i18n-testing';
import { AuthLayout } from './auth-layout';

@Component({
  imports: [AuthLayout],
  template: `<ui-auth-layout><h1>Sign in</h1><p>Form</p></ui-auth-layout>`,
})
class Page {}

beforeEach(() => localStorage.clear());

describe('AuthLayout', () => {
  it('frames the page with the product name and the language choice', async () => {
    TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
    const fixture = TestBed.createComponent(Page);
    await fixture.whenStable();
    const element = fixture.nativeElement as HTMLElement;

    expect(element.querySelector('.brand-name')?.textContent).toBe('My Little Big Project Planner');
    expect(element.querySelector('main h1')?.textContent).toBe('Sign in');
    const languages = Array.from(element.querySelectorAll<HTMLButtonElement>('nav button'));
    expect(languages.map((b) => b.textContent?.trim())).toEqual(['Français', 'English']);
    expect(languages.map((b) => b.getAttribute('aria-pressed'))).toEqual(['false', 'true']);
    expect(element.querySelector('nav')?.getAttribute('aria-label')).toBe('Language');

    languages[0].click();
    await fixture.whenStable();
    expect(TestBed.inject(LocaleService).language()).toBe('fr');
    expect(languages[0].getAttribute('aria-pressed')).toBe('true');
    expect(await axeViolations(element)).toEqual([]);
  });
});

describe('AccountMenu', () => {
  it('names the member and signs out', async () => {
    TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
    const fixture = TestBed.createComponent(AccountMenu);
    fixture.componentRef.setInput('displayName', 'Lucas Bernard');
    fixture.componentRef.setInput('email', 'lucas@acme.example');
    let signedOut = 0;
    fixture.componentInstance.signOut.subscribe(() => signedOut++);
    await fixture.whenStable();

    const menu = await TestbedHarnessEnvironment.loader(fixture).getHarness(MatMenuHarness);
    expect(await (await menu.host()).getAttribute('aria-label')).toBe('Account: Lucas Bernard');
    await menu.open();
    const panel = document.querySelector('.mat-mdc-menu-panel') as HTMLElement;
    expect(panel.textContent).toContain('Lucas Bernard');
    expect(panel.textContent).toContain('lucas@acme.example');
    await menu.clickItem({ text: /Sign out/ });
    expect(signedOut).toBe(1);
  });
});
