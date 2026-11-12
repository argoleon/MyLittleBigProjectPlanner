import { Component, inject } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialogModule } from '@angular/material/dialog';
import { TranslocoPipe } from '@jsverse/transloco';

/** What a confirmation asks. Keys are translation keys; `params` fill them. */
export interface ConfirmDialogData {
  readonly titleKey: string;
  readonly messageKey: string;
  readonly confirmKey: string;
  readonly params?: Readonly<Record<string, string>>;
}

/**
 * Asks to confirm an action that cannot be undone. Closes with `true` when
 * confirmed:
 *
 * ```ts
 * dialog.open(ConfirmDialog, { data }).afterClosed().subscribe((yes) => …);
 * ```
 */
@Component({
  selector: 'ui-confirm-dialog',
  imports: [MatButtonModule, MatDialogModule, TranslocoPipe],
  template: `
    <h2 mat-dialog-title>{{ data.titleKey | transloco: data.params }}</h2>
    <mat-dialog-content>
      <p>{{ data.messageKey | transloco: data.params }}</p>
    </mat-dialog-content>
    <mat-dialog-actions align="end">
      <button matButton type="button" [mat-dialog-close]="false">{{ 'dialog.cancel' | transloco }}</button>
      <button matButton="filled" type="button" [mat-dialog-close]="true">{{ data.confirmKey | transloco }}</button>
    </mat-dialog-actions>
  `,
})
export class ConfirmDialog {
  protected readonly data = inject<ConfirmDialogData>(MAT_DIALOG_DATA);
}
