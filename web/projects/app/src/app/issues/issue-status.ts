import { Component, inject, input, output, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatMenuModule } from '@angular/material/menu';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  Issue,
  IssueTransition,
  StatusChip,
  errorTranslationKey,
  isApiProblem,
  listIssueTransitions,
  runIssueTransition,
} from 'ui';

import { chipCategory } from './issue-fields';
import { TransitionDialog, TransitionDialogData, TransitionOutcome } from './transition-dialog';

/**
 * The status of a ticket and what moves it (mockup 03, L05-US09): for those
 * who edit, a menu of the transitions they may run, the first also as a
 * filled button; a transition that takes fields shows them first.
 */
@Component({
  selector: 'app-issue-status',
  imports: [MatButtonModule, MatIconModule, MatMenuModule, StatusChip, TranslocoPipe],
  templateUrl: './issue-status.html',
  styleUrl: './issue-status.scss',
})
export class IssueStatus {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);

  readonly issue = input.required<Issue>();
  readonly canEdit = input(false);
  /** The ticket moved: in its new status. */
  readonly saved = output<Issue>();
  /** Someone changed it first: read it again. */
  readonly reload = output();

  protected readonly chipCategory = chipCategory;
  /** Read again for each version of the ticket. */
  protected readonly transitions = resource({
    params: () => (this.canEdit() ? { idOrKey: this.issue().id, version: this.issue().version } : undefined),
    loader: ({ params }) => this.api.invoke(listIssueTransitions, { idOrKey: params.idOrKey }),
  });
  protected readonly running = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  protected readonly conflict = signal(false);

  protected available(): IssueTransition[] {
    return this.transitions.hasValue() ? this.transitions.value() : [];
  }

  protected async run(transition: IssueTransition): Promise<void> {
    this.errorKey.set(null);
    this.conflict.set(false);
    if (transition.fields.length > 0) {
      const data: TransitionDialogData = { ticket: this.issue(), transition };
      this.dialog
        .open<TransitionDialog, TransitionDialogData, TransitionOutcome>(TransitionDialog, {
          data,
          width: 'min(560px, 100vw)',
          autoFocus: 'first-tabbable',
        })
        .afterClosed()
        .subscribe((outcome) => {
          if (outcome && 'saved' in outcome) this.saved.emit(outcome.saved);
          else if (outcome) this.reload.emit();
        });
      return;
    }
    this.running.set(true);
    try {
      const saved = await this.api.invoke(runIssueTransition, {
        idOrKey: this.issue().id,
        'If-Match': `"${this.issue().version}"`,
        body: { transitionId: transition.id },
      });
      this.saved.emit(saved);
    } catch (error) {
      if (isApiProblem(error) && error.status === 412) this.conflict.set(true);
      else this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
    } finally {
      this.running.set(false);
    }
  }
}
