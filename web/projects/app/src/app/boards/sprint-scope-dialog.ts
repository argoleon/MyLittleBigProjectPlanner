import { Component, inject, resource } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialogModule } from '@angular/material/dialog';
import { MatTableModule } from '@angular/material/table';
import { RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, BoardEstimation, ErrorState, LoadingState, LocalNumberPipe, SprintScopeItem, getSprintScope } from 'ui';

/** The sprint whose scope to show, and the board's estimation. */
export interface SprintScopeData {
  readonly sprintId: string;
  readonly estimation: BoardEstimation;
}

/** What became of a ticket of the sprint: done, not done, taken out, or still running. */
export function outcomeOf(item: SprintScopeItem): 'done' | 'notDone' | 'removed' | 'running' {
  if (item.removed) return 'removed';
  if (item.completed === true) return 'done';
  if (item.completed === false) return 'notDone';
  return 'running';
}

/**
 * The scope of a sprint over its life (L09-US02): what it committed to when
 * it started and completed when it closed, then each ticket — committed or
 * added later, done, not done or taken out — with its estimates then.
 */
@Component({
  selector: 'app-sprint-scope-dialog',
  imports: [ErrorState, LoadingState, LocalNumberPipe, MatButtonModule, MatDialogModule, MatTableModule, RouterLink, TranslocoPipe],
  styleUrl: './sprint-scope-dialog.scss',
  templateUrl: './sprint-scope-dialog.html',
})
export class SprintScopeDialog {
  protected readonly data = inject<SprintScopeData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  protected readonly columns = ['key', 'summary', 'arrival', 'outcome', 'estimate'];
  protected readonly outcomeOf = outcomeOf;

  protected readonly scope = resource({
    params: () => this.data.sprintId,
    loader: ({ params }) => this.api.invoke(getSprintScope, { id: params }),
  });
}
