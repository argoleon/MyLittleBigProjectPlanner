import { Component, computed, inject, input, output, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Absence,
  Api,
  ConfirmDialog,
  ConfirmDialogData,
  DurationPipe,
  ErrorState,
  LocalDatePipe,
  deleteAbsence,
  errorTranslationKey,
  isApiProblem,
  listAbsences,
  toLocalDate,
} from 'ui';

import { AbsenceDialog, AbsenceDialogData } from './absence-dialog';

/** How long an absence lasts, as the card says it: one day, or a range. */
export type AbsenceSpan =
  | { readonly single: true; readonly part: 'whole' | 'morning' | 'afternoon' }
  | { readonly single: false; readonly fromNoon: boolean; readonly toNoon: boolean };

/** The span of `absence`. */
export function absenceSpan(absence: Pick<Absence, 'start' | 'end' | 'startsAtNoon' | 'endsAtNoon'>): AbsenceSpan {
  if (absence.start === absence.end) {
    return { single: true, part: absence.startsAtNoon ? 'afternoon' : absence.endsAtNoon ? 'morning' : 'whole' };
  }
  return { single: false, fromNoon: absence.startsAtNoon, toNoon: absence.endsAtNoon };
}

/** The window of absences the card shows: a month back, the rest of the year ahead. */
export function absenceWindow(today: Date): { from: string; to: string } {
  const shift = (days: number) => {
    const date = new Date(today);
    date.setDate(date.getDate() + days);
    return toLocalDate(date);
  };
  return { from: shift(-30), to: shift(334) };
}

/**
 * The absences of a resource (mockup 20, L07-US05), from a month ago to a
 * year ahead: dates, half days, kind, who declared them, and the comment
 * for the person and the managers. They declare, change and delete them.
 * An absence on hours planned in hours per day or total effort shows the
 * hours in conflict, to reschedule (L08-US04).
 */
@Component({
  selector: 'app-absences-card',
  imports: [DurationPipe, ErrorState, LocalDatePipe, MatButtonModule, MatIconModule, MatProgressBarModule, TranslocoPipe],
  templateUrl: './absences-card.html',
  styleUrl: './absences-card.scss',
})
export class AbsencesCard {
  /** The resource. */
  readonly resourceId = input.required<string>();
  /** An absence was declared, changed or deleted. */
  readonly changed = output<void>();

  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);

  private readonly window = absenceWindow(new Date());
  protected readonly list = resource({
    params: () => ({ id: this.resourceId(), ...this.window }),
    loader: ({ params }) => this.api.invoke(listAbsences, params),
  });
  protected readonly absences = computed<readonly Absence[]>(() => (this.list.hasValue() ? this.list.value().items : []));
  readonly canManage = computed(() => this.list.hasValue() && this.list.value().canManage);
  protected readonly notice = signal<{ ok: boolean; messageKey: string } | null>(null);
  protected readonly span = absenceSpan;

  protected actionId(absence: Absence, action: 'edit' | 'delete'): string {
    return `absence-${action}-${absence.id}`;
  }

  /** Declares an absence; `restoreFocus` is the button that opened it. */
  declare(restoreFocus = '#absence-new'): void {
    this.open({ resourceId: this.resourceId() }, restoreFocus, 'absences.created');
  }

  protected edit(absence: Absence): void {
    this.open({ resourceId: this.resourceId(), absence }, `#${this.actionId(absence, 'edit')}`, 'absences.saved');
  }

  protected remove(absence: Absence): void {
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, {
        data: { titleKey: 'absences.deleteTitle', messageKey: 'absences.deleteMessage', confirmKey: 'absences.deleteConfirm' },
        width: 'min(480px, 100vw)',
        restoreFocus: `#${this.actionId(absence, 'delete')}`,
      })
      .afterClosed()
      .subscribe(async (confirmed) => {
        if (!confirmed) return;
        try {
          await this.api.invoke(deleteAbsence, { id: this.resourceId(), absenceId: absence.id, 'If-Match': `"${absence.version}"` });
          this.done('absences.deleted');
        } catch (error) {
          this.notice.set({ ok: false, messageKey: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
        }
      });
  }

  private open(data: AbsenceDialogData, restoreFocus: string, messageKey: string): void {
    this.dialog
      .open<AbsenceDialog, AbsenceDialogData, Absence>(AbsenceDialog, {
        data,
        width: 'min(520px, 100vw)',
        autoFocus: 'first-tabbable',
        restoreFocus,
      })
      .afterClosed()
      .subscribe((saved) => {
        if (saved) this.done(messageKey);
      });
  }

  private done(messageKey: string): void {
    this.notice.set({ ok: true, messageKey });
    this.list.reload();
    this.changed.emit();
  }
}
