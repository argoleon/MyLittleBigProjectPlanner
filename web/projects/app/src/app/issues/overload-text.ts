import { OverloadDay, formatDuration, formatLocalDate } from 'ui';

/** Overloaded days spelled out before the rest are counted. */
export const DAYS_SPELLED_OUT = 3;

/** Translates a key with its parameters. */
export type Translate = (key: string, params?: Record<string, string>) => string;

/**
 * The overloaded `days` in words, in `locale`: `Wed 18 Nov: 8 h planned for
 * 7 h available (+1 h)`, the first {@link DAYS_SPELLED_OUT} spelled out and
 * the others counted.
 */
export function overloadDaysText(days: readonly OverloadDay[], locale: string, translate: Translate): string {
  const spelled = days.slice(0, DAYS_SPELLED_OUT).map((day) =>
    translate('assignments.overload.day', {
      date: formatLocalDate(day.date, locale, { weekday: 'short', day: 'numeric', month: 'short' }),
      planned: formatDuration(day.plannedMinutes, locale),
      available: formatDuration(day.availableMinutes, locale),
      over: formatDuration(day.plannedMinutes - day.availableMinutes, locale),
    }),
  );
  const rest = days.length - spelled.length;
  if (rest > 0) {
    spelled.push(translate(rest === 1 ? 'assignments.overload.moreOne' : 'assignments.overload.moreMany', { count: String(rest) }));
  }
  return spelled.join(translate('assignments.overload.separator'));
}
