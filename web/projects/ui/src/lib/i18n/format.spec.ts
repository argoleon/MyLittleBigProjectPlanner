import {
  formatDuration,
  formatFileSize,
  formatInstant,
  formatLocalDate,
  formatNumber,
  fromLocalDate,
  parseHours,
  parseLocalDate,
  toLocalDate,
  weekdayNames,
} from './format';

// Time zones on both sides of UTC, with and without daylight saving time.
const ZONES = ['Europe/Paris', 'America/Los_Angeles', 'Pacific/Kiritimati', 'Pacific/Pago_Pago', 'UTC'];

describe('parseLocalDate', () => {
  it('splits valid dates', () => {
    expect(parseLocalDate('2026-03-29')).toEqual({ year: 2026, month: 3, day: 29 });
    expect(parseLocalDate('2028-02-29')).toEqual({ year: 2028, month: 2, day: 29 });
  });

  it('rejects anything else', () => {
    for (const bad of ['2026-02-30', '2027-02-29', '2026-13-01', '2026-3-29', '29/03/2026', '2026-03-29T00:00:00Z', '']) {
      expect(parseLocalDate(bad), bad).toBeNull();
    }
  });
});

describe('formatLocalDate', () => {
  it('keeps the calendar day whatever the time zone, including on DST change days', () => {
    // 2026-03-29 and 2026-10-25: DST changes in Europe; 2026-03-08 and
    // 2026-11-01: in the United States.
    for (const day of ['2026-03-29', '2026-10-25', '2026-03-08', '2026-11-01', '2026-01-01', '2026-12-31']) {
      const expected = Number(day.slice(8));
      for (const zone of ZONES) {
        const text = formatLocalDate(day, 'en-GB', { day: 'numeric', timeZone: zone });
        expect(Number(text), `${day} in ${zone}`).toBe(expected);
      }
    }
  });

  it('formats in the language of the user', () => {
    expect(formatLocalDate('2026-03-29', 'fr', { dateStyle: 'long' })).toBe('29 mars 2026');
    expect(formatLocalDate('2026-03-29', 'en-GB', { dateStyle: 'long' })).toBe('29 March 2026');
    expect(formatLocalDate('2026-03-29', 'en-US', { dateStyle: 'short' })).toBe('3/29/26');
  });

  it('refuses instants and invalid dates', () => {
    expect(() => formatLocalDate('2026-03-29T10:00:00Z', 'fr')).toThrow(RangeError);
    expect(() => formatLocalDate('2026-02-30', 'fr')).toThrow(RangeError);
  });
});

describe('formatInstant', () => {
  const time = { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' } as const;

  it('shows instants in the time zone of the user, across DST changes', () => {
    // Europe/Paris moves from UTC+1 to UTC+2 at 01:00 UTC on 2026-03-29.
    expect(formatInstant('2026-03-29T00:30:00Z', 'fr', 'Europe/Paris', time)).toBe('01:30');
    expect(formatInstant('2026-03-29T01:30:00Z', 'fr', 'Europe/Paris', time)).toBe('03:30');
    // And back to UTC+1 at 01:00 UTC on 2026-10-25.
    expect(formatInstant('2026-10-25T00:30:00Z', 'fr', 'Europe/Paris', time)).toBe('02:30');
    expect(formatInstant('2026-10-25T01:30:00Z', 'fr', 'Europe/Paris', time)).toBe('02:30');
    // The same instant on two sides of the date line.
    expect(formatInstant('2026-11-09T23:00:00Z', 'en-GB', 'Pacific/Kiritimati', { dateStyle: 'short' })).toBe('10/11/2026');
    expect(formatInstant('2026-11-09T23:00:00Z', 'en-GB', 'America/Los_Angeles', { dateStyle: 'short' })).toBe('09/11/2026');
  });

  it('accepts Date objects and refuses invalid values', () => {
    expect(formatInstant(new Date(Date.UTC(2026, 10, 9, 12)), 'fr', 'UTC', time)).toBe('12:00');
    expect(() => formatInstant('not a date', 'fr', 'UTC')).toThrow(RangeError);
  });
});

describe('formatNumber', () => {
  it('uses the separators of the locale', () => {
    // French groups with a narrow no-break space.
    expect(formatNumber(1234.5, 'fr')).toBe('1 234,5');
    expect(formatNumber(1234.5, 'en-GB')).toBe('1,234.5');
    expect(formatNumber(0.875, 'fr', { style: 'percent', maximumFractionDigits: 1 })).toBe('87,5 %');
  });
});

describe('toLocalDate and fromLocalDate', () => {
  it('go between the date picker and local dates without converting through UTC', () => {
    // Midnight local time: in UTC, still the day before east of Greenwich.
    expect(toLocalDate(new Date(2026, 2, 29))).toBe('2026-03-29');
    expect(toLocalDate(new Date(2026, 0, 5, 23, 59))).toBe('2026-01-05');
    const date = fromLocalDate('2026-03-29')!;
    expect([date.getFullYear(), date.getMonth(), date.getDate()]).toEqual([2026, 2, 29]);
    expect(toLocalDate(date)).toBe('2026-03-29');
  });

  it('give nothing for an empty or invalid date', () => {
    expect(fromLocalDate(null)).toBeNull();
    expect(fromLocalDate('')).toBeNull();
    expect(fromLocalDate('2026-02-30')).toBeNull();
  });
});

describe('formatFileSize', () => {
  it('says sizes in the units people read, in their language', () => {
    expect(formatFileSize(512, 'en')).toBe('512 bytes');
    expect(formatFileSize(3_145_728, 'en')).toBe('3.1 MB');
    expect(formatFileSize(3_145_728, 'fr')).toMatch(/^3,1\s?Mo$/);
    expect(formatFileSize(52_428_800, 'en')).toBe('52.4 MB');
    expect(formatFileSize(1500, 'en')).toBe('1.5 kB');
  });
});

describe('formatDuration', () => {
  it('says hours and minutes the way planners read them', () => {
    expect(formatDuration(420, 'fr')).toBe('7 h');
    expect(formatDuration(210, 'fr')).toBe('3 h 30');
    expect(formatDuration(210, 'en')).toBe('3 h 30 min');
    expect(formatDuration(5, 'fr')).toBe('0 h 05');
    expect(formatDuration(1680, 'en')).toBe('28 h');
    expect(formatDuration(0, 'fr')).toBe('0 h');
  });
});

describe('parseHours', () => {
  it('reads decimal hours with a comma or a dot, and hours with minutes', () => {
    expect(parseHours('7')).toBe(420);
    expect(parseHours('3,5')).toBe(210);
    expect(parseHours('3.5')).toBe(210);
    expect(parseHours('3:30')).toBe(210);
    expect(parseHours(' 3 h 30 ')).toBe(210);
    expect(parseHours('3h30')).toBe(210);
    expect(parseHours('7 h')).toBe(420);
    expect(parseHours('0,25')).toBe(15);
    expect(parseHours('')).toBe(0);
    expect(parseHours('24')).toBe(1440);
  });

  it('refuses what is not a duration of a day at most', () => {
    for (const text of ['abc', '25', '3:75', '-2', '3,5,5', '24:01', '1.5.']) {
      expect(parseHours(text), text).toBeNull();
    }
  });
});

describe('weekdayNames', () => {
  it('names the days Monday first, in the language', () => {
    expect(weekdayNames('fr')).toEqual(['lundi', 'mardi', 'mercredi', 'jeudi', 'vendredi', 'samedi', 'dimanche']);
    expect(weekdayNames('en', 'short')).toEqual(['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']);
  });
});
