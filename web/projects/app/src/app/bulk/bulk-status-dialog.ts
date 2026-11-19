import { Component, inject, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatRadioModule } from '@angular/material/radio';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, BulkAction, ErrorState, createBulkAction, errorTranslationKey, getBulkOptions, isApiProblem } from 'ui';

import { BulkDialogData } from './bulk-flow';

/**
 * Changes the status of the selected tickets (L06-US08): only the statuses
 * a transition reaches from theirs are offered, with how many tickets it
 * reaches; each ticket runs its own workflow's transition, those it cannot
 * reach reported.
 */
@Component({
  selector: 'app-bulk-status-dialog',
  imports: [ErrorState, MatButtonModule, MatDialogModule, MatProgressBarModule, MatRadioModule, TranslocoPipe],
  template: `
    <h2 mat-dialog-title>{{ 'bulk.status.title' | transloco: { count: data.count } }}</h2>
    <mat-dialog-content>
      @if (errorKey(); as key) {
        <p class="message error" role="alert">{{ key | transloco }}</p>
      }
      @if (options.error()) {
        <ui-error-state [error]="options.error()" [headingLevel]="3" (retry)="options.reload()" />
      } @else if (!options.hasValue()) {
        <mat-progress-bar mode="indeterminate" [attr.aria-label]="'state.loading' | transloco" />
      } @else if (options.value().statuses.length === 0) {
        <p class="note">{{ 'bulk.status.none' | transloco }}</p>
      } @else {
        <p class="note" id="bulk-status-label">{{ 'bulk.status.hint' | transloco }}</p>
        <mat-radio-group class="statuses" aria-labelledby="bulk-status-label" [value]="chosen()" (change)="chosen.set($event.value)">
          @for (option of options.value().statuses; track option.key) {
            <mat-radio-button [value]="option.key">
              {{ option.label ?? option.key }}
              <span class="reach">{{ 'bulk.status.reach' | transloco: { count: option.reachable, total: options.value().total } }}</span>
            </mat-radio-button>
          }
        </mat-radio-group>
      }
    </mat-dialog-content>
    <mat-dialog-actions align="end">
      <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
      <button matButton="filled" type="button" class="confirm" [disabled]="!chosen() || running()" (click)="run()">
        {{ 'bulk.status.confirm' | transloco }}
      </button>
    </mat-dialog-actions>
  `,
  styleUrl: './bulk-dialog.scss',
})
export class BulkStatusDialog {
  protected readonly data = inject<BulkDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<BulkStatusDialog, BulkAction>>(MatDialogRef);

  protected readonly options = resource({
    loader: () => this.api.invoke(getBulkOptions, { body: this.data.selection }),
  });
  protected readonly chosen = signal<string | null>(null);
  protected readonly running = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  protected async run(): Promise<void> {
    const statusKey = this.chosen();
    if (!statusKey) return;
    this.running.set(true);
    this.errorKey.set(null);
    try {
      const done = await this.api.invoke(createBulkAction, {
        body: { selection: this.data.selection, action: { kind: 'transition', statusKey } },
      });
      this.dialogRef.close(done);
    } catch (error) {
      this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
    } finally {
      this.running.set(false);
    }
  }
}
