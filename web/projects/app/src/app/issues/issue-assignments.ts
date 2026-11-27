import { Component, computed, effect, inject, input, resource, signal, untracked } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { TranslocoPipe, TranslocoService } from '@jsverse/transloco';
import {
  Api,
  Assignment,
  AssignmentSegment,
  Avatar,
  ConfirmDialog,
  ConfirmDialogData,
  DurationPipe,
  ErrorState,
  Issue,
  LocalDatePipe,
  LocaleService,
  deleteAssignment,
  errorTranslationKey,
  isApiProblem,
  listIssueAssignments,
} from 'ui';

import { AssignmentDialog, AssignmentDialogData } from './assignment-dialog';
import { overloadDaysText } from './overload-text';
import { ProvisionalConfirmation } from './provisional-confirmation';
import { segmentValueText } from './segment-text';

/** The first and last days of `assignments`; null when none has a segment. */
export function planSpan(assignments: readonly Assignment[]): { from: string; to: string } | null {
  const segments = assignments.flatMap((a) => a.segments);
  if (segments.length === 0) return null;
  const from = segments.map((s) => s.start).reduce((a, b) => (a < b ? a : b));
  const to = segments.map((s) => s.end).reduce((a, b) => (a > b ? a : b));
  return { from, to };
}

/**
 * The assignments of a ticket (mockup 03, L08-US01): one block per
 * resource with its segments — period, mode, value and load — and the
 * ticket's planned total, the sum of its assignments. Those who assign
 * resources add, change and remove them through the dialog of mockup 21.
 * An assignment taking part in an overload of its resource is flagged with
 * its days (L08-US03). A provisional assignment is confirmed, then checked
 * for overload as a firm one (L08-US09).
 */
@Component({
  selector: 'app-issue-assignments',
  imports: [Avatar, DurationPipe, ErrorState, LocalDatePipe, MatButtonModule, MatIconModule, MatProgressBarModule, TranslocoPipe],
  templateUrl: './issue-assignments.html',
  styleUrl: './issue-assignments.scss',
})
export class IssueAssignments {
  /** The ticket. */
  readonly issue = input.required<Issue>();
  /** Changes when the ticket changed elsewhere: its assignments are read again. */
  readonly refresh = input(0);

  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly locale = inject(LocaleService);
  private readonly transloco = inject(TranslocoService);
  private readonly confirmation = inject(ProvisionalConfirmation);

  protected readonly list = resource({
    params: () => this.issue().id,
    loader: ({ params }) => this.api.invoke(listIssueAssignments, { idOrKey: params }),
  });
  protected readonly items = computed<readonly Assignment[]>(() => (this.list.hasValue() ? (this.list.value().items ?? []) : []));
  protected readonly total = computed(() => (this.list.hasValue() ? (this.list.value().loadMinutes ?? 0) : 0));
  protected readonly canAssign = computed(() => this.list.hasValue() && this.list.value().canAssign === true);
  protected readonly span = computed(() => planSpan(this.items()));
  protected readonly notice = signal<{ ok: boolean; messageKey: string; params?: Record<string, string> } | null>(null);

  constructor() {
    let first = true;
    effect(() => {
      this.refresh();
      if (first) {
        first = false;
        return;
      }
      untracked(() => this.list.reload());
    });
  }

  /** What a segment's value reads: `50 %`, `4 h/day`, `40 h over 19 working days`. */
  protected valueText(segment: AssignmentSegment): string {
    return segmentValueText(segment, this.locale.formattingLocale(), (key, params) => this.transloco.translate(key, params));
  }

  /** The minutes of an assignment an absence leaves no room for. */
  protected conflictMinutes(assignment: Assignment): number {
    return assignment.conflicts.reduce((sum, conflict) => sum + conflict.minutes, 0);
  }

  /** The days an assignment overloads, in words. */
  protected overloadText(assignment: Assignment): string {
    return overloadDaysText(assignment.overloads, this.locale.formattingLocale(), (key, params) => this.transloco.translate(key, params));
  }

  protected add(): void {
    this.open({ issue: this.issue() }, '#assignment-add');
  }

  protected edit(assignment: Assignment): void {
    this.open({ issue: this.issue(), assignment }, `#assignment-edit-${assignment.id}`);
  }

  private open(data: AssignmentDialogData, restoreFocus: string): void {
    this.dialog
      .open<AssignmentDialog, AssignmentDialogData, Assignment>(AssignmentDialog, {
        data,
        width: 'min(880px, 100vw)',
        autoFocus: 'first-tabbable',
        restoreFocus,
      })
      .afterClosed()
      .subscribe((saved) => {
        if (!saved) return;
        this.notice.set({
          ok: true,
          messageKey: data.assignment ? 'assignments.saved' : 'assignments.created',
          params: { name: saved.resource.displayName },
        });
        this.list.reload();
      });
  }

  /** Makes a provisional assignment firm, forcing its overload when asked. */
  protected async confirm(assignment: Assignment): Promise<void> {
    this.notice.set(null);
    const outcome = await this.confirmation.confirm(assignment.id, assignment.version, `#assignment-confirm-${assignment.id}`);
    if (outcome.status === 'cancelled') return;
    if (outcome.status === 'refused') {
      this.notice.set({ ok: false, messageKey: outcome.messageKey, params: outcome.params });
      return;
    }
    this.notice.set({ ok: true, messageKey: 'assignments.confirm.done', params: { name: assignment.resource.displayName } });
    this.list.reload();
  }

  protected remove(assignment: Assignment): void {
    const name = assignment.resource.displayName;
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, {
        data: { titleKey: 'assignments.deleteTitle', messageKey: 'assignments.deleteMessage', confirmKey: 'assignments.deleteConfirm', params: { name } },
        width: 'min(480px, 100vw)',
        restoreFocus: '#assignment-add',
      })
      .afterClosed()
      .subscribe(async (confirmed) => {
        if (!confirmed) return;
        try {
          await this.api.invoke(deleteAssignment, { id: assignment.id, 'If-Match': `"${assignment.version}"` });
          this.notice.set({ ok: true, messageKey: 'assignments.deleted', params: { name } });
          this.list.reload();
        } catch (error) {
          this.notice.set({ ok: false, messageKey: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
        }
      });
  }
}
