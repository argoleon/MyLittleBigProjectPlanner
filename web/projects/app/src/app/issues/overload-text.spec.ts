/**
 * Tests of the overloaded days in words (L08-US03): each day with what is
 * planned and available, the first ones spelled out and the rest counted,
 * in the reader's language.
 */
import { TestBed } from '@angular/core/testing';
import { TranslocoService } from '@jsverse/transloco';
import { LocaleService, OverloadDay } from 'ui';
import { provideI18nTesting } from 'ui/testing';

import { overloadDaysText } from './overload-text';

function day(date: string, plannedMinutes: number, availableMinutes = 420): OverloadDay {
  return { date, plannedMinutes, availableMinutes, limitMinutes: availableMinutes };
}

function say(days: OverloadDay[], lang: 'en' | 'fr'): string {
  localStorage.clear();
  TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
  TestBed.inject(LocaleService).use(lang);
  const transloco = TestBed.inject(TranslocoService);
  return overloadDaysText(days, lang, (key, params) => transloco.translate(key, params));
}

describe('overloadDaysText', () => {
  // `use('fr')` remembers the language in the browser: forget it, for the specs that follow.
  afterEach(() => {
    localStorage.clear();
    TestBed.resetTestingModule();
  });

  it('says each day with its planned and available hours', () => {
    expect(say([day('2026-11-18', 480)], 'en')).toBe('Wed, Nov 18: 8 h planned for 7 h available (+1 h)');
    TestBed.resetTestingModule();
    expect(say([day('2026-11-18', 480), day('2026-11-20', 240, 210)], 'fr')).toBe(
      'mer. 18 nov. : 8 h planifiées pour 7 h disponibles (+1 h) ; ven. 20 nov. : 4 h planifiées pour 3 h 30 disponibles (+0 h 30)',
    );
  });

  it('spells out the first three days and counts the others', () => {
    const days = ['2026-11-16', '2026-11-17', '2026-11-18', '2026-11-19', '2026-11-20'].map((d) => day(d, 450));
    expect(say(days.slice(0, 4), 'en')).toMatch(/\(\+0 h 30 min\); and 1 more day$/);
    TestBed.resetTestingModule();
    expect(say(days, 'en')).toMatch(/^Mon, Nov 16: .*; and 2 more days$/);
  });
});
