import { Component, computed, inject, resource, signal } from '@angular/core';
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
  ConfirmDialog,
  ConfirmDialogData,
  ErrorState,
  deleteCalendar,
  errorTranslationKey,
  isApiProblem,
  listCalendars,
} from 'ui';

import { CalendarDialog, CalendarDialogData } from './calendar-dialog';

/** Outcome of the last action. */
interface Notice {
  readonly ok: boolean;
  readonly messageKey: string;
  readonly params?: Record<string, string>;
}

/**
 * "Calendars" tab of Workload (specification §7, L07-US04): the
 * organization's calendar and the site calendars, each opening its year.
 * Every internal member reads them; tenant admins and resource managers
 * create, change and delete sites.
 */
@Component({
  selector: 'app-calendars-page',
  imports: [ErrorState, MatButtonModule, MatIconModule, MatProgressBarModule, MatTableModule, RouterLink, TranslocoPipe],
  templateUrl: './calendars-page.html',
  styleUrl: '../teams/teams-page.scss',
})
export class CalendarsPage {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);

  protected readonly list = resource({ loader: () => this.api.invoke(listCalendars) });
  protected readonly calendars = computed<readonly Calendar[]>(() => (this.list.hasValue() ? this.list.value().items : []));
  protected readonly canManage = computed(() => this.list.hasValue() && this.list.value().canManage);
  protected readonly columns = computed(() => ['name', 'holidays', 'resources', ...(this.canManage() ? ['actions'] : [])]);
  protected readonly notice = signal<Notice | null>(null);
  protected readonly trackCalendar = (_: number, calendar: Calendar) => calendar.id;

  protected actionsId(calendar: Calendar, action: 'edit' | 'delete'): string {
    return `calendar-${action}-${calendar.id}`;
  }

  protected create(): void {
    this.open({}, undefined, 'calendars.created');
  }

  protected edit(calendar: Calendar): void {
    this.open({ calendar }, `#${this.actionsId(calendar, 'edit')}`, 'calendars.saved');
  }

  protected remove(calendar: Calendar): void {
    const name = calendar.name ?? '';
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, {
        data: { titleKey: 'calendars.deleteTitle', messageKey: 'calendars.deleteMessage', confirmKey: 'calendars.deleteConfirm', params: { name } },
        width: 'min(480px, 100vw)',
        restoreFocus: `#${this.actionsId(calendar, 'delete')}`,
      })
      .afterClosed()
      .subscribe(async (confirmed) => {
        if (!confirmed) return;
        try {
          await this.api.invoke(deleteCalendar, { id: calendar.id, 'If-Match': `"${calendar.version}"` });
          this.notice.set({ ok: true, messageKey: 'calendars.deleted', params: { name } });
          this.list.reload();
        } catch (error) {
          this.notice.set({ ok: false, messageKey: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
        }
      });
  }

  private open(data: CalendarDialogData, restoreFocus: string | undefined, messageKey: string): void {
    this.dialog
      .open<CalendarDialog, CalendarDialogData, Calendar>(CalendarDialog, {
        data,
        width: 'min(560px, 100vw)',
        autoFocus: 'first-tabbable',
        restoreFocus: restoreFocus ?? true,
      })
      .afterClosed()
      .subscribe((saved) => {
        if (!saved) return;
        this.notice.set({ ok: true, messageKey, params: { name: saved.name ?? '' } });
        this.list.reload();
      });
  }
}
