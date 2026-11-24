import { Component, ElementRef, computed, inject, input, resource, signal, viewChild } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { TranslocoPipe, TranslocoService } from '@jsverse/transloco';
import {
  Api,
  CalendarException,
  ConfirmDialog,
  ConfirmDialogData,
  DurationPipe,
  ErrorState,
  LocalDatePipe,
  LocaleService,
  ResourceDay,
  deleteResourceException,
  errorTranslationKey,
  formatLocalDate,
  isApiProblem,
  listResourceDays,
  listResourceExceptions,
  toLocalDate,
  weekdayNames,
} from 'ui';

import { ExceptionDialog, ExceptionDialogData } from './exception-dialog';

/** How a day looks: worked, not worked (weekend, 0 h), closed, or a public holiday. */
export type DayLook = 'working' | 'off' | 'closed' | 'holiday';

/** The look of `day`. */
export function dayLook(day: ResourceDay): DayLook {
  if (day.status === 'holiday') return 'holiday';
  if (day.status === 'non_working') return 'closed';
  return day.minutes === 0 ? 'off' : 'working';
}

/** The first day of the month of `date` (`YYYY-MM-DD`), and its last. */
export function monthBounds(year: number, month: number): { from: string; to: string } {
  const last = new Date(Date.UTC(year, month + 1, 0)).getUTCDate();
  const pad = (n: number) => String(n).padStart(2, '0');
  return { from: `${year}-${pad(month + 1)}-01`, to: `${year}-${pad(month + 1)}-${pad(last)}` };
}

/** The weeks of a month, Monday first: `null` before the 1st and after the last day. */
export function monthWeeks<T>(days: readonly T[], firstWeekday: number): (T | null)[][] {
  const cells: (T | null)[] = [...Array<null>(firstWeekday).fill(null), ...days];
  while (cells.length % 7 !== 0) cells.push(null);
  return Array.from({ length: cells.length / 7 }, (_, week) => cells.slice(week * 7, week * 7 + 7));
}

/**
 * The month of a resource (mockup 20, L07-US04): each day worked or not —
 * public holidays, closures, days of no capacity — with a legend. The grid
 * moves with the arrow keys (Home, End, Page Up, Page Down change month);
 * each day has an accessible name. Managers click (or press Enter on) a
 * day to add or change the resource's own exception that day.
 */
@Component({
  selector: 'app-month-calendar',
  imports: [DurationPipe, ErrorState, LocalDatePipe, MatButtonModule, MatIconModule, MatProgressBarModule, TranslocoPipe],
  templateUrl: './month-calendar.html',
  styleUrl: './month-calendar.scss',
})
export class MonthCalendar {
  /** The resource. */
  readonly resourceId = input.required<string>();
  /** The member manages its exceptions. */
  readonly canManage = input(false);

  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly locale = inject(LocaleService);
  private readonly transloco = inject(TranslocoService);
  private readonly grid = viewChild<ElementRef<HTMLElement>>('grid');

  private readonly today = toLocalDate(new Date());
  /** The month shown: year and month (0–11). */
  protected readonly month = signal({ year: new Date().getFullYear(), month: new Date().getMonth() });
  private readonly bounds = computed(() => monthBounds(this.month().year, this.month().month));
  protected readonly title = computed(() =>
    formatLocalDate(this.bounds().from, this.locale.formattingLocale(), { month: 'long', year: 'numeric' }),
  );

  protected readonly days = resource({
    params: () => ({ id: this.resourceId(), ...this.bounds() }),
    loader: ({ params }) => this.api.invoke(listResourceDays, params),
  });
  protected readonly exceptions = resource({
    params: () => ({ id: this.resourceId(), ...this.bounds() }),
    loader: ({ params }) => this.api.invoke(listResourceExceptions, params),
  });
  protected readonly weekdays = computed(() => weekdayNames(this.locale.formattingLocale(), 'short'));
  protected readonly weekdayLong = computed(() => weekdayNames(this.locale.formattingLocale()));
  protected readonly weeks = computed(() => {
    if (!this.days.hasValue()) return [];
    const first = new Date(`${this.bounds().from}T00:00:00Z`).getUTCDay();
    return monthWeeks(this.days.value(), (first + 6) % 7);
  });
  protected readonly ownExceptions = computed<readonly CalendarException[]>(() =>
    this.exceptions.hasValue() ? this.exceptions.value().items : [],
  );
  /** The day that holds the focus in the grid. */
  protected readonly focused = signal<string | null>(null);
  protected readonly notice = signal<{ ok: boolean; messageKey: string } | null>(null);

  protected readonly look = dayLook;

  protected dayNumber(day: ResourceDay): number {
    return Number(day.day.slice(8));
  }

  protected isToday(day: ResourceDay): boolean {
    return day.day === this.today;
  }

  /** The day taking part in the tab order: the focused one, else today, else the 1st. */
  protected tabbable(day: ResourceDay): boolean {
    const days = this.days.hasValue() ? this.days.value() : [];
    const target = this.focused() ?? (days.some((d) => d.day === this.today) ? this.today : days[0]?.day);
    return day.day === target;
  }

  /**
   * What a screen reader hears after the visible text of a day (its number
   * and tag), so that the name holds what is seen (WCAG 2.5.3): the full
   * date, what the day is, and whether the resource's own exception set it.
   */
  protected dayDetail(day: ResourceDay): string {
    const date = formatLocalDate(day.day, this.locale.formattingLocale(), { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' });
    const parts = [date, this.transloco.translate(`calendars.look.${dayLook(day)}`)];
    if (day.source === 'resource') parts.push(this.transloco.translate('calendars.ownException'));
    return parts.join(', ');
  }

  protected shift(months: number): void {
    const { year, month } = this.month();
    const next = new Date(Date.UTC(year, month + months, 1));
    this.month.set({ year: next.getUTCFullYear(), month: next.getUTCMonth() });
    this.focused.set(null);
  }

  /** Arrow keys move by a day or a week, Home and End to the ends of the week, Page Up and Down change month. */
  protected onKey(event: KeyboardEvent, day: ResourceDay): void {
    const moves: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -7, ArrowDown: 7 };
    const weekday = (new Date(`${day.day}T00:00:00Z`).getUTCDay() + 6) % 7;
    if (event.key in moves) {
      this.focusDay(day.day, moves[event.key]);
    } else if (event.key === 'Home') {
      this.focusDay(day.day, -weekday);
    } else if (event.key === 'End') {
      this.focusDay(day.day, 6 - weekday);
    } else if (event.key === 'PageUp' || event.key === 'PageDown') {
      this.shift(event.key === 'PageUp' ? -1 : 1);
    } else if ((event.key === 'Enter' || event.key === ' ') && this.canManage()) {
      this.open(day);
    } else {
      return;
    }
    event.preventDefault();
  }

  private focusDay(from: string, offset: number): void {
    const date = new Date(`${from}T00:00:00Z`);
    date.setUTCDate(date.getUTCDate() + offset);
    const target = date.toISOString().slice(0, 10);
    const { year, month } = this.month();
    if (date.getUTCFullYear() !== year || date.getUTCMonth() !== month) {
      this.month.set({ year: date.getUTCFullYear(), month: date.getUTCMonth() });
    }
    this.focused.set(target);
    // The cell exists once the month is drawn.
    queueMicrotask(() => setTimeout(() => this.grid()?.nativeElement.querySelector<HTMLElement>(`[data-day="${target}"]`)?.focus()));
  }

  /** Adds the resource's exception on `day`, or changes the one it has. */
  protected open(day: ResourceDay): void {
    if (!this.canManage()) return;
    const exception = this.ownExceptions().find((e) => e.day === day.day);
    this.focused.set(day.day);
    this.dialog
      .open<ExceptionDialog, ExceptionDialogData, CalendarException>(ExceptionDialog, {
        data: { owner: { resourceId: this.resourceId() }, exception, day: day.day },
        width: 'min(520px, 100vw)',
        autoFocus: 'first-tabbable',
        restoreFocus: `[data-day="${day.day}"]`,
      })
      .afterClosed()
      .subscribe((saved) => {
        if (!saved) return;
        this.notice.set({ ok: true, messageKey: exception ? 'calendars.exception.saved' : 'calendars.exception.created' });
        this.reload();
      });
  }

  protected remove(exception: CalendarException): void {
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, {
        data: { titleKey: 'calendars.exception.deleteTitle', messageKey: 'calendars.exception.deleteMessage', confirmKey: 'calendars.deleteConfirm' },
        width: 'min(480px, 100vw)',
        restoreFocus: `#own-exception-${exception.id}`,
      })
      .afterClosed()
      .subscribe(async (confirmed) => {
        if (!confirmed) return;
        try {
          await this.api.invoke(deleteResourceException, { id: this.resourceId(), exceptionId: exception.id, 'If-Match': `"${exception.version}"` });
          this.notice.set({ ok: true, messageKey: 'calendars.exception.deleted' });
          this.reload();
        } catch (error) {
          this.notice.set({ ok: false, messageKey: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
        }
      });
  }

  private reload(): void {
    this.days.reload();
    this.exceptions.reload();
  }
}
