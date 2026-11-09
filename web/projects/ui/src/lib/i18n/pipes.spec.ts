import { Component } from '@angular/core';
import { TestBed } from '@angular/core/testing';

import { provideI18nTesting } from '../testing/i18n-testing';
import { LocaleService } from './locale.service';
import { InstantPipe, LocalDatePipe, LocalNumberPipe } from './pipes';

@Component({
  imports: [InstantPipe, LocalDatePipe, LocalNumberPipe],
  template: `
    <p id="date">{{ '2026-03-29' | localDate: { dateStyle: 'long' } }}</p>
    <p id="number">{{ 1234.5 | localNumber }}</p>
    <p id="instant">{{ '2026-11-09T12:00:00Z' | instant: { year: 'numeric' } }}</p>
    <p id="empty">{{ null | localDate }}{{ undefined | instant }}{{ null | localNumber }}</p>
  `,
})
class Host {}

describe('formatting pipes', () => {
  beforeEach(() => localStorage.clear());

  it('format in the current language and follow its changes', async () => {
    TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
    const fixture = TestBed.createComponent(Host);
    const text = (id: string) => (fixture.nativeElement as HTMLElement).querySelector(`#${id}`)?.textContent?.trim();

    await fixture.whenStable();
    expect(text('date')).toBe('March 29, 2026');
    expect(text('number')).toBe('1,234.5');
    expect(text('instant')).toBe('2026');
    expect(text('empty')).toBe('');

    TestBed.inject(LocaleService).use('fr');
    fixture.changeDetectorRef.markForCheck();
    await fixture.whenStable();
    expect(text('date')).toBe('29 mars 2026');
    expect(text('number')).toBe('1 234,5');
  });
});
