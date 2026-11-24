import { Component, computed, inject, input, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatTableModule } from '@angular/material/table';
import { RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  Calendar,
  CalendarException,
  ConfirmDialog,
  ConfirmDialogData,
  ErrorState,
  LoadingState,
  LocalDatePipe,
  deleteCalendarException,
  errorTranslationKey,
  getCalendar,
  getCalendarYear,
  isApiProblem,
  listCalendars,
} from 'ui';

import { CalendarDialog, CalendarDialogData } from './calendar-dialog';
import { ExceptionDialog, ExceptionDialogData } from './exception-dialog';

/** A day of the year's table: a public holiday of the set, or an exception. */
export interface YearRow {
  readonly day: string;
  /** `holiday` for the set's; else the exception's kind. */
  readonly kind: 'set' | CalendarException['kind'];
  /** Key of the set's holiday. */
  readonly holiday?: string;
  readonly exception?: CalendarException;
}

/** The rows of a year: holidays and exceptions by date, an exception after the holiday it changes. */
export function yearRows(holidays: readonly { day: string; name: string }[], exceptions: readonly CalendarException[]): YearRow[] {
  const rows: YearRow[] = [
    ...holidays.map((h) => ({ day: h.day, kind: 'set' as const, holiday: h.name })),
    ...exceptions.map((exception) => ({ day: exception.day, kind: exception.kind, exception })),
  ];
  return rows.sort((a, b) => a.day.localeCompare(b.day) || (a.kind === 'set' ? -1 : 1));
}

/**
 * The page of a calendar (L07-US04): its public holidays and exceptions
 * for a year. Tenant admins and resource managers add, change and delete
 * exceptions, and change the calendar.
 */
@Component({
  selector: 'app-calendar-page',
  imports: [ErrorState, LoadingState, LocalDatePipe, MatButtonModule, MatIconModule, MatProgressBarModule, MatTableModule, RouterLink, TranslocoPipe],
  templateUrl: './calendar-page.html',
  styleUrl: './calendar-page.scss',
})
export class CalendarPage {
  /** `:id` of the route. */
  readonly id = input.required<string>();

  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);

  protected readonly year = signal(new Date().getFullYear());
  protected readonly current = resource({
    params: () => this.id(),
    loader: ({ params }) => this.api.invoke(getCalendar, { id: params }),
  });
  /** Whether the member manages calendars: the list says so. */
  private readonly rights = resource({ loader: () => this.api.invoke(listCalendars) });
  protected readonly canManage = computed(() => this.rights.hasValue() && this.rights.value().canManage);
  protected readonly days = resource({
    params: () => ({ id: this.id(), year: this.year() }),
    loader: ({ params }) => this.api.invoke(getCalendarYear, params),
  });
  protected readonly rows = computed<YearRow[]>(() =>
    this.days.hasValue() ? yearRows(this.days.value().holidays, this.days.value().exceptions) : [],
  );
  protected readonly columns = computed(() => ['day', 'kind', 'label', ...(this.canManage() ? ['actions'] : [])]);
  protected readonly notice = signal<{ ok: boolean; messageKey: string } | null>(null);
  protected readonly trackRow = (_: number, row: YearRow) => `${row.day}-${row.kind}`;

  protected actionsId(exception: CalendarException, action: 'edit' | 'delete'): string {
    return `exception-${action}-${exception.id}`;
  }

  protected editCalendar(calendar: Calendar): void {
    this.dialog
      .open<CalendarDialog, CalendarDialogData, Calendar>(CalendarDialog, {
        data: { calendar },
        width: 'min(560px, 100vw)',
        autoFocus: 'first-tabbable',
        restoreFocus: '#calendar-edit',
      })
      .afterClosed()
      .subscribe((saved) => {
        if (!saved) return;
        this.current.reload();
        this.days.reload();
      });
  }

  protected add(): void {
    this.openException({ owner: { calendarId: this.id() }, day: `${this.year()}-01-01` }, '#exception-add', 'calendars.exception.created');
  }

  protected edit(exception: CalendarException): void {
    this.openException({ owner: { calendarId: this.id() }, exception }, `#${this.actionsId(exception, 'edit')}`, 'calendars.exception.saved');
  }

  protected remove(exception: CalendarException): void {
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, {
        data: { titleKey: 'calendars.exception.deleteTitle', messageKey: 'calendars.exception.deleteMessage', confirmKey: 'calendars.deleteConfirm' },
        width: 'min(480px, 100vw)',
        restoreFocus: `#${this.actionsId(exception, 'delete')}`,
      })
      .afterClosed()
      .subscribe(async (confirmed) => {
        if (!confirmed) return;
        try {
          await this.api.invoke(deleteCalendarException, { id: this.id(), exceptionId: exception.id, 'If-Match': `"${exception.version}"` });
          this.notice.set({ ok: true, messageKey: 'calendars.exception.deleted' });
          this.days.reload();
        } catch (error) {
          this.notice.set({ ok: false, messageKey: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
        }
      });
  }

  private openException(data: ExceptionDialogData, restoreFocus: string, messageKey: string): void {
    this.dialog
      .open<ExceptionDialog, ExceptionDialogData, CalendarException>(ExceptionDialog, {
        data,
        width: 'min(520px, 100vw)',
        autoFocus: 'first-tabbable',
        restoreFocus,
      })
      .afterClosed()
      .subscribe((saved) => {
        if (!saved) return;
        this.notice.set({ ok: true, messageKey });
        this.year.set(Number(saved.day.slice(0, 4)));
        this.days.reload();
      });
  }
}
