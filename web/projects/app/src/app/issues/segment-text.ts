import { AssignmentSegment, formatDuration, formatNumber } from 'ui';

import { Translate } from './overload-text';

/** What a segment's value reads, in `locale`: `50 %`, `4 h/day`, `40 h over 19 working days`. */
export function segmentValueText(segment: Pick<AssignmentSegment, 'mode' | 'value' | 'workingDays'>, locale: string, translate: Translate): string {
  switch (segment.mode) {
    case 'percentage':
      return formatNumber(segment.value / 100, locale, { style: 'percent' });
    case 'hours_per_day':
      return translate('assignments.perDay', { hours: formatDuration(segment.value, locale) });
    case 'total_effort':
      return translate('assignments.over', {
        hours: formatDuration(segment.value, locale),
        days: formatNumber(segment.workingDays, locale),
      });
  }
}
