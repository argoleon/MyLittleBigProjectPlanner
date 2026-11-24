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

/**
 * A size in bytes as people read it in `locale`: `3.2 MB`, `3,2 Mo`. Units
 * of 1,000 (kilobyte, megabyte, gigabyte), one decimal at most.
 */
export function formatFileSize(bytes: number, locale: string): string {
  const units = ['byte', 'kilobyte', 'megabyte', 'gigabyte'] as const;
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  return new Intl.NumberFormat(locale, {
    style: 'unit',
    unit: units[unit],
    unitDisplay: unit === 0 ? 'long' : 'short',
    maximumFractionDigits: unit === 0 ? 0 : 1,
  }).format(value);
}

/**
 * A duration of `minutes` in hours and minutes, the way planners read it:
 * `7 h`, `3 h 30` (`3 h 30 min` in English). Hours are formatted in
 * `locale`, so a week of 1,680 minutes reads `28 h`.
 */
export function formatDuration(minutes: number, locale: string): string {
  const whole = Math.round(minutes);
  const hours = Math.trunc(whole / 60);
  const rest = Math.abs(whole % 60);
  const h = `${formatNumber(hours, locale)} h`;
  if (rest === 0) return h;
  const mm = String(rest).padStart(2, '0');
  return locale.startsWith('fr') ? `${h} ${mm}` : `${h} ${mm} min`;
}

const DECIMAL_HOURS = /^(\d{1,2})(?:[.,](\d{1,4}))?\s*h?$/i;
const HOURS_MINUTES = /^(\d{1,2})\s*(?:h|:)\s*(\d{1,2})\s*(?:min|m)?$/i;

/**
 * The minutes of a duration typed in hours: `7`, `3,5`, `3.5`, `3:30`,
 * `3 h 30`; empty is 0. `null` when it reads as none of these, or exceeds a
 * day.
 */
export function parseHours(text: string): number | null {
  const value = text.trim();
  if (value === '') return 0;
  let minutes: number | null = null;
  const decimal = DECIMAL_HOURS.exec(value);
  const clock = HOURS_MINUTES.exec(value);
  if (decimal) {
    minutes = Math.round(Number(`${decimal[1]}.${decimal[2] ?? '0'}`) * 60);
  } else if (clock && Number(clock[2]) < 60) {
    minutes = Number(clock[1]) * 60 + Number(clock[2]);
  }
  return minutes !== null && minutes <= 24 * 60 ? minutes : null;
}

/**
 * The names of the days of the week in `locale`, Monday first: `lundi`…
 * (`long`) or `lun.`… (`short`).
 */
export function weekdayNames(locale: string, style: 'long' | 'short' = 'long'): string[] {
  const format = new Intl.DateTimeFormat(locale, { weekday: style, timeZone: 'UTC' });
  // 2024-01-01 was a Monday.
  return Array.from({ length: 7 }, (_, day) => format.format(new Date(Date.UTC(2024, 0, 1 + day))));
}
