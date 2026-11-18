import { Component, inject } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { TranslocoPipe } from '@jsverse/transloco';
import { IssueTransition } from 'ui';

/** The transitions that lead a card to the column it was dropped in. */
export interface ChooseTransitionData {
  readonly key: string;
  readonly transitions: readonly IssueTransition[];
}

/**
 * Asks which transition to run when several lead a card to the column it
 * was dropped in (L06-US05). Closes with the one picked; cancelled, the
 * card goes back.
 */
@Component({
  selector: 'app-choose-transition-dialog',
  imports: [MatButtonModule, MatDialogModule, TranslocoPipe],
  template: `
    <h2 mat-dialog-title>{{ 'boards.choose.title' | transloco: { key: data.key } }}</h2>
    <mat-dialog-content>
      <p class="note">{{ 'boards.choose.message' | transloco }}</p>
      <div class="choices">
        @for (transition of data.transitions; track transition.id) {
          <button matButton="tonal" type="button" class="choice" (click)="pick(transition)">
            {{ transition.name }} → {{ transition.toStatus.label ?? transition.toStatus.key }}
          </button>
        }
      </div>
    </mat-dialog-content>
    <mat-dialog-actions align="end">
      <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
    </mat-dialog-actions>
  `,
  styleUrl: './choose-transition-dialog.scss',
})
export class ChooseTransitionDialog {
  protected readonly data = inject<ChooseTransitionData>(MAT_DIALOG_DATA);
  private readonly dialogRef = inject<MatDialogRef<ChooseTransitionDialog, IssueTransition>>(MatDialogRef);

  protected pick(transition: IssueTransition): void {
    this.dialogRef.close(transition);
  }
}
