import { TestBed } from '@angular/core/testing';
import { TranslocoService } from '@jsverse/transloco';

import { provideI18nTesting } from '../testing/i18n-testing';
import { LocaleService, formattingLocale, isLanguage, languageFrom } from './locale.service';

describe('language helpers', () => {
  it('picks the first supported browser language', () => {
    expect(languageFrom(['de-DE', 'en-GB', 'fr'])).toBe('en');
    expect(languageFrom(['FR-ca'])).toBe('fr');
    expect(languageFrom(['de', 'es'])).toBe('fr');
    expect(languageFrom([])).toBe('fr');
  });

  it('keeps the browser region for formatting when it matches', () => {
    expect(formattingLocale('en', ['fr-FR', 'en-GB'])).toBe('en-GB');
    expect(formattingLocale('fr', ['en-GB'])).toBe('fr');
  });

  it('recognises languages', () => {
    expect(isLanguage('en')).toBe(true);
    expect(isLanguage('de')).toBe(false);
    expect(isLanguage(null)).toBe(false);
  });
});

describe('LocaleService', () => {
  beforeEach(() => localStorage.clear());

  function create(): LocaleService {
    TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
    return TestBed.inject(LocaleService);
  }

  it('starts from the browser language (jsdom: en-US)', () => {
    const locale = create();
    expect(locale.language()).toBe('en');
    expect(locale.formattingLocale()).toBe('en-US');
    expect(document.documentElement.lang).toBe('en');
    expect(TestBed.inject(TranslocoService).getActiveLang()).toBe('en');
  });

  it('prefers the language the user chose before', () => {
    localStorage.setItem('mlbpp.language', 'fr');
    expect(create().language()).toBe('fr');
  });

  it('ignores an invalid stored language', () => {
    localStorage.setItem('mlbpp.language', 'klingon');
    expect(create().language()).toBe('en');
  });

  it('switches language everywhere and remembers it', () => {
    const locale = create();
    locale.use('fr');
    expect(locale.language()).toBe('fr');
    expect(locale.formattingLocale()).toBe('fr');
    expect(document.documentElement.lang).toBe('fr');
    expect(TestBed.inject(TranslocoService).translate('nav.workload')).toBe('Charge');
    expect(localStorage.getItem('mlbpp.language')).toBe('fr');
  });

  it('exposes the browser time zone', () => {
    expect(create().timeZone()).toBe(Intl.DateTimeFormat().resolvedOptions().timeZone);
  });
});
