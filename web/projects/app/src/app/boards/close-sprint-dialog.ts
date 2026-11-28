import { Component, computed, inject, linkedSignal, resource, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatRadioModule } from '@angular/material/radio';
import { MatSelectModule } from '@angular/material/select';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, Sprint, SprintCloseTarget, SprintClosed, closeSprint, errorTranslationKey, isApiProblem, listSprints } from 'ui';

/** The sprint to close, and how many of its tickets are done or not, as the screen shows them. */
export interface CloseSprintData {
  readonly sprint: Sprint;
  readonly done: number;
  readonly unfinished: number;
}

/**
 * Closes the active sprint of a Scrum board (L09-US02): its finished tickets
 * stay its completed scope; the member chooses where the unfinished ones go
 * — the backlog, a future sprint of the board (the next one first), or a new
 * sprint. Closes with what the close did.
 */
@Component({
  selector: 'app-close-sprint-dialog',
  imports: [FormsModule, MatButtonModule, MatDialogModule, MatFormFieldModule, MatRadioModule, MatSelectModule, TranslocoPipe],
  styleUrl: './close-sprint-dialog.scss',
  templateUrl: './close-sprint-dialog.html',
})
export class CloseSprintDialog {
  protected readonly data = inject<CloseSprintData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<CloseSprintDialog, SprintClosed>>(MatDialogRef);

  /** The future sprints of the board, the next one first. */
  protected readonly future = resource({
    params: () => this.data.sprint.boardId,
    loader: async ({ params }) => (await this.api.invoke(listSprints, { id: params, state: 'open' })).items.filter((s) => s.state === 'future'),
  });
  /** Where the unfinished tickets go: the next sprint by default, when there is one. */
  protected readonly choice = linkedSignal<SprintCloseTarget>(() =>
    this.future.hasValue() && this.future.value().length > 0 ? 'sprint' : 'backlog',
  );
  protected readonly target = signal<string | null>(null);
  /** The future sprint chosen: the one picked, else the next one. */
  protected readonly targetId = computed(() => this.target() ?? (this.future.hasValue() ? (this.future.value()[0]?.id ?? null) : null));
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  protected async close(): Promise<void> {
    if (this.saving()) return;
    const moveTo = this.data.unfinished > 0 ? this.choice() : 'backlog';
    const sprintId = moveTo === 'sprint' ? this.targetId() : null;
    if (moveTo === 'sprint' && !sprintId) return;
    this.saving.set(true);
    this.errorKey.set(null);
    try {
      const closed = await this.api.invoke(closeSprint, {
        id: this.data.sprint.id,
        'If-Match': `"${this.data.sprint.version}"`,
        body: { moveTo, sprintId },
      });
      this.dialogRef.close(closed);
    } catch (error) {
      this.errorKey.set(!isApiProblem(error) ? errorTranslationKey('unknown') : error.status === 412 ? 'backlog.close.conflict' : error.translationKey);
    } finally {
      this.saving.set(false);
    }
  }
}
