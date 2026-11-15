import { Component, inject, signal } from '@angular/core';
import { AbstractControl, FormControl, FormRecord, ReactiveFormsModule } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { provideNativeDateAdapter } from '@angular/material/core';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, Issue, IssueTransition, getIssue, isApiProblem, runIssueTransition } from 'ui';

import { placeApiErrors } from '../configuration/form-errors';
import { FieldInput } from './field-input';
import { FieldRaw, apiValue, controlOf, rawOf } from './issue-fields';

/** A transition to run on a ticket. */
export interface TransitionDialogData {
  readonly ticket: Issue;
  readonly transition: IssueTransition;
}

/** How a transition ends: the ticket in its new status, or a reload asked for after a conflict. */
export type TransitionOutcome = { readonly saved: Issue } | { readonly reload: true };

/**
 * Runs a transition that takes fields (L05-US09): those of its screen, its
 * validators and those required at the target status, prefilled with the
 * ticket's values; refusals shown next to their fields.
 */
@Component({
  selector: 'app-transition-dialog',
  imports: [FieldInput, MatButtonModule, MatDialogModule, ReactiveFormsModule, TranslocoPipe],
  providers: [provideNativeDateAdapter()],
  template: `
    <h2 mat-dialog-title>{{ data.transition.name }}</h2>
    <mat-dialog-content class="fields">
      @if (conflict()) {
        <div class="message error conflict" role="alert">
          <p>{{ 'issue.edit.conflict' | transloco }}</p>
          <button matButton="tonal" type="button" class="reload" (click)="reload()">{{ 'issue.edit.reload' | transloco }}</button>
        </div>
      }
      @if (errorKey(); as key) {
        <p class="message error" role="alert">{{ key | transloco }}</p>
      }
      <p class="note">{{ 'issue.transition.to' | transloco: { status: data.transition.toStatus.label ?? data.transition.toStatus.key } }}</p>
      @for (field of data.transition.fields; track field.fieldId) {
        <app-field-input [field]="field" [control]="controls.controls[field.fieldId]" [projectKey]="data.ticket.project.key" />
      }
    </mat-dialog-content>
    <mat-dialog-actions align="end">
      <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
      <button matButton="filled" type="button" class="confirm" [disabled]="saving() || conflict()" (click)="run()">
        {{ data.transition.name }}
      </button>
    </mat-dialog-actions>
  `,
  styleUrl: './issue-dialog.scss',
})
export class TransitionDialog {
  protected readonly data = inject<TransitionDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<TransitionDialog, TransitionOutcome>>(MatDialogRef);

  protected readonly controls = new FormRecord<FormControl<FieldRaw>>({});
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  protected readonly conflict = signal(false);

  constructor() {
    for (const field of this.data.transition.fields) {
      const control = controlOf(field);
      control.setValue(rawOf(field, this.data.ticket.fields.find((v) => v.fieldId === field.fieldId)));
      this.controls.addControl(field.fieldId, control);
    }
  }

  protected reload(): void {
    this.dialogRef.close({ reload: true });
  }

  protected async run(): Promise<void> {
    if (this.controls.invalid) {
      this.controls.markAllAsTouched();
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    try {
      const fields = await this.changed();
      if (!fields) return;
      const saved = await this.api.invoke(runIssueTransition, {
        idOrKey: this.data.ticket.id,
        'If-Match': `"${this.data.ticket.version}"`,
        body: { transitionId: this.data.transition.id, fields },
      });
      this.dialogRef.close({ saved });
    } catch (error) {
      if (isApiProblem(error) && error.status === 412) {
        this.conflict.set(true);
        return;
      }
      const controls: Record<string, AbstractControl> = {};
      for (const [id, control] of Object.entries(this.controls.controls)) controls[`fields.${id}`] = control;
      this.errorKey.set(placeApiErrors(error, controls));
    } finally {
      this.saving.set(false);
    }
  }

  /** The values edited, `null` for those cleared, ticket keys resolved to ids; null if a key is unknown. */
  private async changed(): Promise<Record<string, unknown> | null> {
    const values: Record<string, unknown> = {};
    let unknown = false;
    for (const field of this.data.transition.fields) {
      const control = this.controls.controls[field.fieldId];
      if (!control.dirty) continue;
      let value = apiValue(field, control.value) ?? null;
      if (field.kind === 'issue_link' && value !== null) {
        try {
          value = (await this.api.invoke(getIssue, { idOrKey: value as string })).id;
        } catch {
          control.setErrors({ api: 'errors.field.unknown_issue' });
          control.markAsTouched();
          unknown = true;
          continue;
        }
      }
      values[field.fieldId] = value;
    }
    return unknown ? null : values;
  }
}
