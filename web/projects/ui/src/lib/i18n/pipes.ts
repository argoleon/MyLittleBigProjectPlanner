import { Pipe, PipeTransform, inject } from '@angular/core';

import { formatDuration, formatInstant, formatLocalDate, formatNumber } from './format';
import { LocaleService } from './locale.service';

// Not pure: they follow language changes. Formatting is cheap.

/** `{{ item.createdAt | instant }}`: an instant in the user's time zone. */
@Pipe({ name: 'instant', pure: false })
export class InstantPipe implements PipeTransform {
  private readonly locale = inject(LocaleService);

  transform(value: string | Date | null | undefined, options?: Intl.DateTimeFormatOptions): string {
    if (value === null || value === undefined || value === '') {
      return '';
    }
    return formatInstant(value, this.locale.formattingLocale(), this.locale.timeZone(), options);
  }
}

/** `{{ task.startDate | localDate }}`: a planning date, never converted. */
@Pipe({ name: 'localDate', pure: false })
export class LocalDatePipe implements PipeTransform {
  private readonly locale = inject(LocaleService);

  transform(value: string | null | undefined, options?: Intl.DateTimeFormatOptions): string {
    if (!value) {
      return '';
    }
    return formatLocalDate(value, this.locale.formattingLocale(), options);
  }
}

/** `{{ hours | localNumber }}`: a number in the user's locale. */
@Pipe({ name: 'localNumber', pure: false })
export class LocalNumberPipe implements PipeTransform {
  private readonly locale = inject(LocaleService);

  transform(value: number | null | undefined, options?: Intl.NumberFormatOptions): string {
    if (value === null || value === undefined) {
      return '';
    }
    return formatNumber(value, this.locale.formattingLocale(), options);
  }
}

/** `{{ day.minutes | duration }}`: minutes as `7 h` or `3 h 30`. */
@Pipe({ name: 'duration', pure: false })
export class DurationPipe implements PipeTransform {
  private readonly locale = inject(LocaleService);

  transform(minutes: number | null | undefined): string {
    if (minutes === null || minutes === undefined) {
      return '';
    }
    return formatDuration(minutes, this.locale.formattingLocale());
  }
}
