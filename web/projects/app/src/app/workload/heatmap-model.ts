import {
  Heatmap as HeatmapData,
  HeatmapCellData,
  HeatmapColumnHeader,
  HeatmapLevel,
  HeatmapPeriod,
  HeatmapRowHeader,
  HeatmapScale,
  addDays,
  formatDuration,
  formatLocalDate,
  formatNumber,
  isoWeek,
  mondayOf,
} from 'ui';

/** Translates a key with its parameters. */
export type Translate = (key: string, params?: Record<string, string>) => string;

/** Columns shown at once in each scale: four weeks of days, eight weeks, six months. */
export const COLUMNS: Readonly<Record<HeatmapScale, number>> = { day: 28, week: 8, month: 6 };

/** The first day of the month of a local date. */
function firstOfMonth(value: string): string {
  return `${value.slice(0, 8)}01`;
}

/** The first day of the month `months` after that of `value`. */
function addMonths(value: string, months: number): string {
  const [year, month] = value.split('-').map(Number);
  const date = new Date(Date.UTC(year, month - 1 + months, 1));
  return date.toISOString().slice(0, 10);
}

/** The first column's start for `scale` around `day`: its Monday, or the first of its month. */
export function anchorOf(scale: HeatmapScale, day: string): string {
  return scale === 'month' ? firstOfMonth(day) : mondayOf(day);
}

/** The days the heatmap reads from `anchor` in `scale`: first and last, included. */
export function rangeOf(scale: HeatmapScale, anchor: string): { from: string; to: string } {
  switch (scale) {
    case 'day':
      return { from: anchor, to: addDays(anchor, COLUMNS.day - 1) };
    case 'week':
      return { from: anchor, to: addDays(anchor, COLUMNS.week * 7 - 1) };
    case 'month':
      return { from: anchor, to: addDays(addMonths(anchor, COLUMNS.month), -1) };
  }
}

/** `anchor` moved by `steps` screens (back when negative). */
export function shift(scale: HeatmapScale, anchor: string, steps: number): string {
  switch (scale) {
    case 'day':
      return addDays(anchor, steps * COLUMNS.day);
    case 'week':
      return addDays(anchor, steps * COLUMNS.week * 7);
    case 'month':
      return addMonths(anchor, steps * COLUMNS.month);
  }
}

/**
 * The level of a period (as `domain::workload::level`): beyond the
 * capacity plus `tolerance` percent it overloads; without capacity it is
 * off, or absent when absences took scheduled days.
 */
export function levelOf(load: number, capacity: number, scheduled: number, tolerance: number): HeatmapLevel {
  const limit = Math.floor((capacity * (100 + tolerance)) / 100);
  if (load > limit) return 'over';
  if (capacity === 0) return scheduled === 0 ? 'off' : 'absent';
  if (load === 0) return 'free';
  const percent = Math.floor((load * 100) / capacity);
  if (percent <= 40) return 'low';
  if (percent <= 70) return 'medium';
  if (percent <= 95) return 'high';
  return 'full';
}

/** The load in percent of the capacity, rounded; `null` without capacity. */
export function percentOf(load: number, capacity: number): number | null {
  return capacity > 0 ? Math.round((load * 100) / capacity) : null;
}

/** A period's header in `scale`. */
export function columnOf(scale: HeatmapScale, period: HeatmapPeriod, locale: string, translate: Translate): HeatmapColumnHeader {
  switch (scale) {
    case 'day':
      return {
        label: formatLocalDate(period.start, locale, { weekday: 'short' }),
        sublabel: formatLocalDate(period.start, locale, { day: 'numeric', month: 'short' }),
      };
    case 'week':
      return {
        label: translate('workload.heatmap.week', { number: String(isoWeek(period.start)) }),
        sublabel: translate('workload.heatmap.weekSpan', {
          from: formatLocalDate(period.start, locale, { day: 'numeric' }),
          to: formatLocalDate(period.end, locale, { day: 'numeric', month: 'short' }),
        }),
      };
    case 'month':
      return {
        label: formatLocalDate(period.start, locale, { month: 'short' }),
        sublabel: formatLocalDate(period.start, locale, { year: 'numeric' }),
      };
  }
}

/** What the heatmap draws, and what the table view lists. */
export interface HeatmapModel {
  readonly rows: readonly HeatmapRowHeader[];
  readonly columns: readonly HeatmapColumnHeader[];
  /** By row then column; the last row is the averages. */
  readonly cells: readonly (readonly HeatmapCellData[])[];
  /** Cells of resources over the capacity plus the tolerance. */
  readonly overloads: number;
}

/**
 * The model of `data`: a row per resource, its cells with their level,
 * percentage and description, and a last row of averages (over the cells
 * with capacity).
 */
export function buildModel(data: HeatmapData, locale: string, translate: Translate): HeatmapModel {
  const percent = (value: number) => formatNumber(value / 100, locale, { style: 'percent' });
  const columns = data.periods.map((period) => columnOf(data.scale, period, locale, translate));
  let overloads = 0;
  const cells = data.rows.map((row) =>
    data.periods.map((_, i): HeatmapCellData => {
      const load = row.load[i] ?? 0;
      const capacity = row.capacity[i] ?? 0;
      const scheduled = row.scheduled[i] ?? 0;
      const level = levelOf(load, capacity, scheduled, data.tolerancePercent);
      if (level === 'over') overloads++;
      const share = percentOf(load, capacity);
      const hidden = (row.hidden[i] ?? 0) > 0;
      const provisional = (row.provisional[i] ?? 0) > 0;
      const text =
        level === 'absent'
          ? translate('workload.heatmap.absent')
          : level === 'off'
            ? '–'
            : share === null
              ? formatDuration(load, locale)
              : percent(share);
      const parts = [
        level === 'absent'
          ? translate('workload.heatmap.describeAbsent')
          : level === 'off'
            ? translate('workload.heatmap.describeOff')
            : translate('workload.heatmap.describe', {
                percent: share === null ? '–' : percent(share),
                planned: formatDuration(load, locale),
                available: formatDuration(capacity, locale),
              }),
      ];
      if (level === 'over') parts.push(translate('workload.heatmap.describeOver'));
      if (provisional) parts.push(translate('workload.heatmap.describeProvisional', { hours: formatDuration(row.provisional[i], locale) }));
      if (hidden) parts.push(translate('workload.heatmap.describeHidden', { hours: formatDuration(row.hidden[i], locale) }));
      return {
        level,
        text,
        subtext: hidden ? translate('workload.heatmap.hidden') : provisional ? translate('workload.heatmap.provisional') : undefined,
        provisional: provisional && !hidden,
        hidden,
        description: parts.join(', '),
      };
    }),
  );
  const averages = data.periods.map((_, i): HeatmapCellData => {
    const shares = data.rows.map((row) => percentOf(row.load[i] ?? 0, row.capacity[i] ?? 0)).filter((p): p is number => p !== null);
    const average = shares.length > 0 ? Math.round(shares.reduce((sum, p) => sum + p, 0) / shares.length) : null;
    const text = average === null ? '–' : percent(average);
    return { level: 'summary', text, description: translate('workload.heatmap.describeAverage', { percent: text }) };
  });
  const rows: HeatmapRowHeader[] = data.rows.map((row) => ({
    label: row.resource.displayName,
    sublabel: row.resource.jobTitle ?? translate(`resource.holding.${row.resource.kind === 'generic' ? 'generic' : 'person'}`),
  }));
  return {
    rows: data.rows.length > 0 ? [...rows, { label: translate('workload.heatmap.average') }] : [],
    columns,
    cells: data.rows.length > 0 ? [...cells, averages] : [],
    overloads,
  };
}
