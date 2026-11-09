import { TestbedHarnessEnvironment } from '@angular/cdk/testing/testbed';
import { Component } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { MatMenuHarness } from '@angular/material/menu/testing';

import { LocaleService } from '../i18n/locale.service';
import { provideI18nTesting } from '../testing/i18n-testing';
import { LanguageMenu } from './language-menu';
import { TenantSwitcher } from './tenant-switcher';
import { ThemeMenu } from './theme-menu';

@Component({
  imports: [LanguageMenu, ThemeMenu],
  template: `<ui-language-menu /><ui-theme-menu />`,
})
class Menus {}

async function setUp() {
  localStorage.clear();
  delete document.documentElement.dataset['theme'];
  TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
  const fixture = TestBed.createComponent(Menus);
  await fixture.whenStable();
  const [language, theme] = await TestbedHarnessEnvironment.loader(fixture).getAllHarnesses(MatMenuHarness);
  return { fixture, language, theme };
}

beforeEach(() => localStorage.clear());

describe('ThemeMenu', () => {
  it('offers the three modes as radio items and applies the one picked', async () => {
    const { fixture, theme } = await setUp();
    const button = await theme.host();
    expect(await button.getAttribute('aria-label')).toBe('Theme: System default');

    await theme.open();
    const items = await theme.getItems();
    expect(await Promise.all(items.map((i) => i.getText()))).toEqual([
      'brightness_autoSystem default',
      'light_modeLight',
      'dark_modeDark',
    ]);
    const roles = await Promise.all(items.map(async (i) => (await i.host()).getAttribute('role')));
    expect(roles).toEqual(['menuitemradio', 'menuitemradio', 'menuitemradio']);
    expect(await (await items[0].host()).getAttribute('aria-checked')).toBe('true');

    await items[2].click();
    await fixture.whenStable();
    expect(document.documentElement.dataset['theme']).toBe('dark');
    expect(await button.getAttribute('aria-label')).toBe('Theme: Dark');
    expect(await button.text()).toBe('dark_mode');

    await theme.clickItem({ text: /Light/ });
    expect(document.documentElement.dataset['theme']).toBe('light');
  });
});

describe('LanguageMenu', () => {
  it('lists each language in its own language and marks the current one', async () => {
    const { language } = await setUp();
    await language.open();
    const items = await language.getItems();
    expect(await Promise.all(items.map((i) => i.getText()))).toEqual(['Français', 'checkEnglish']);
    expect(await (await items[0].host()).getAttribute('lang')).toBe('fr');
    expect(await (await items[1].host()).getAttribute('aria-checked')).toBe('true');

    await items[0].click();
    expect(TestBed.inject(LocaleService).language()).toBe('fr');
  });
});

describe('TenantSwitcher', () => {
  it('shows the current tenant', async () => {
    TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
    const fixture = TestBed.createComponent(TenantSwitcher);
    fixture.componentRef.setInput('tenants', [{ id: 't1', name: 'Acme' }]);
    await fixture.whenStable();
    const element = fixture.nativeElement as HTMLElement;
    expect(element.querySelector('mat-label')?.textContent).toBe('Organization');
    expect(element.querySelector('.mat-mdc-select-value')?.textContent?.trim()).toBe('Acme');
    expect(element.querySelector('mat-select')?.getAttribute('aria-disabled')).toBe('false');
  });

  it('is disabled while no tenant is known', async () => {
    TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
    const fixture = TestBed.createComponent(TenantSwitcher);
    await fixture.whenStable();
    expect((fixture.nativeElement as HTMLElement).querySelector('mat-select')?.getAttribute('aria-disabled')).toBe('true');
  });

  it('lets the user pick another tenant when there are several', async () => {
    TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
    const fixture = TestBed.createComponent(TenantSwitcher);
    fixture.componentRef.setInput('tenants', [
      { id: 't1', name: 'Acme' },
      { id: 't2', name: 'Globex' },
    ]);
    fixture.componentRef.setInput('current', 't2');
    const picked: string[] = [];
    fixture.componentInstance.switched.subscribe((id) => picked.push(id));
    await fixture.whenStable();

    const element = fixture.nativeElement as HTMLElement;
    expect(element.querySelector('.mat-mdc-select-value')?.textContent?.trim()).toBe('Globex');
    expect(element.querySelector('mat-select')?.getAttribute('aria-disabled')).toBe('false');
    const { MatSelectHarness } = await import('@angular/material/select/testing');
    const select = await TestbedHarnessEnvironment.loader(fixture).getHarness(MatSelectHarness);
    await select.clickOptions({ text: 'Acme' });
    expect(picked).toEqual(['t1']);
  });
});
