import { addDays, parseLocalDate } from 'ui';

/** Days in a sprint by default: two weeks, the last day included. */
export const DEFAULT_SPRINT_DAYS = 14;

/** Days between two local dates (`YYYY-MM-DD`), `to` minus `from`. */
export function daysBetween(from: string, to: string): number {
  const a = parseLocalDate(from);
  const b = parseLocalDate(to);
  if (!a || !b) return 0;
  const utc = (d: { year: number; month: number; day: number }) => Date.UTC(d.year, d.month - 1, d.day);
  return Math.round((utc(b) - utc(a)) / 86_400_000);
}

/** Where a sprint ending on `end` stands on `today`: days left, its last day, or ended. */
export type Remaining = { readonly kind: 'left'; readonly days: number } | { readonly kind: 'lastDay' } | { readonly kind: 'ended'; readonly days: number };

/** How long the sprint ending on `end` (included) still runs on `today`. */
export function remaining(today: string, end: string): Remaining {
  const days = daysBetween(today, end);
  if (days > 0) return { kind: 'left', days };
  if (days === 0) return { kind: 'lastDay' };
  return { kind: 'ended', days: -days };
}

/** The dates a sprint starts with by default: from `today`, two weeks. */
export function defaultDates(today: string): { start: string; end: string } {
  return { start: today, end: addDays(today, DEFAULT_SPRINT_DAYS - 1) };
}
