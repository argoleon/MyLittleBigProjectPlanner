/**
 * Locale-aware formatting of dates and numbers (specification §19).
 *
 * Two kinds of dates never mix:
 *
 * - **Instants** (`timestamptz` in the API, ISO strings ending in `Z`): a
 *   moment in time, shown in the user's time zone with {@link formatInstant}.
 * - **Local dates** (`date` in the API, `YYYY-MM-DD`): a calendar day of a
 *   resource's planning, shown as is with {@link formatLocalDate}. They never
 *   go through a time zone conversion, so 2026-03-29 stays the 29th for every
 *   user, whatever their time zone and daylight saving time.
 *
 * These functions are pure: the language and time zone are arguments. The
 * pipes (`instant`, `localDate`, `localNumber`) pass the current ones.
 */

const LOCAL_DATE = /^(\d{4})-(\d{2})-(\d{2})$/;

/**
 * The local date (`YYYY-MM-DD`) of `date` as the date picker gives it: the
 * calendar day it shows, never converted through UTC.
 */
export function toLocalDate(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** The date picker's value for a local date (`YYYY-MM-DD`); `null` if invalid. */
export function fromLocalDate(value: string | null | undefined): Date | null {
  const parts = value ? parseLocalDate(value) : null;
  return parts ? new Date(parts.year, parts.month - 1, parts.day) : null;
}

/** A planning date (`YYYY-MM-DD`), split into its parts; `null` if invalid. */
export function parseLocalDate(value: string): { year: number; month: number; day: number } | null {
  const match = LOCAL_DATE.exec(value);
  if (!match) {
    return null;
  }
  const [year, month, day] = [Number(match[1]), Number(match[2]), Number(match[3])];
  // Reject 2026-02-30 and the like: the date must round-trip.
  const probe = new Date(Date.UTC(year, month - 1, day));
  if (probe.getUTCFullYear() !== year || probe.getUTCMonth() !== month - 1 || probe.getUTCDate() !== day) {
    return null;
  }
  return { year, month, day };
}

/**
 * Formats a planning date in `locale`, e.g. `29 mars 2026`. The date is
 * formatted in UTC on purpose: UTC midnight of that day is the same calendar
 * day in UTC, whatever the user's time zone.
 */
export function formatLocalDate(
  value: string,
  locale: string,
  options: Intl.DateTimeFormatOptions = { dateStyle: 'medium' },
): string {
  const date = parseLocalDate(value);
  if (!date) {
    throw new RangeError(`not a local date (YYYY-MM-DD): ${value}`);
  }
  const utc = new Date(Date.UTC(date.year, date.month - 1, date.day));
  return new Intl.DateTimeFormat(locale, { ...options, timeZone: 'UTC' }).format(utc);
}

/** Formats an instant (ISO 8601 with offset, or a `Date`) in `timeZone`. */
export function formatInstant(
  value: string | Date,
  locale: string,
  timeZone: string,
  options: Intl.DateTimeFormatOptions = { dateStyle: 'medium', timeStyle: 'short' },
): string {
  const date = typeof value === 'string' ? new Date(value) : value;
  if (Number.isNaN(date.getTime())) {
    throw new RangeError(`not an instant: ${String(value)}`);
  }
  return new Intl.DateTimeFormat(locale, { ...options, timeZone }).format(date);
}

/** Formats a number in `locale`, e.g. `1 234,5` in French. */
export function formatNumber(value: number, locale: string, options?: Intl.NumberFormatOptions): string {
  return new Intl.NumberFormat(locale, options).format(value);
}
