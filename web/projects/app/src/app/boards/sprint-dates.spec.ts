/**
 * Tests of the sprint dates (L09-US02): days between local dates, what
 * remains of a sprint, and the two weeks a sprint starts with by default.
 */
import { daysBetween, defaultDates, remaining } from './sprint-dates';

describe('the sprint dates', () => {
  it('counts the days between two local dates, across a time change', () => {
    expect(daysBetween('2026-11-30', '2026-12-11')).toBe(11);
    expect(daysBetween('2026-10-24', '2026-10-26')).toBe(2);
    expect(daysBetween('2026-12-11', '2026-11-30')).toBe(-11);
    expect(daysBetween('nope', '2026-11-30')).toBe(0);
  });

  it('tells the days left, the last day, or how long ago it ended', () => {
    expect(remaining('2026-12-08', '2026-12-11')).toEqual({ kind: 'left', days: 3 });
    expect(remaining('2026-12-10', '2026-12-11')).toEqual({ kind: 'left', days: 1 });
    expect(remaining('2026-12-11', '2026-12-11')).toEqual({ kind: 'lastDay' });
    expect(remaining('2026-12-14', '2026-12-11')).toEqual({ kind: 'ended', days: 3 });
  });

  it('starts a sprint today, for two weeks', () => {
    expect(defaultDates('2026-11-30')).toEqual({ start: '2026-11-30', end: '2026-12-13' });
    expect(defaultDates('2026-12-28')).toEqual({ start: '2026-12-28', end: '2027-01-10' });
  });
});
