import { Component, inject, signal } from '@angular/core';
import { FormControl, FormGroup, ReactiveFormsModule } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, Team, TranslatableInput, createTeam, translationsForm, translationsValue, updateTeam } from 'ui';

import { placeApiErrors } from '../configuration/form-errors';

/** What the dialog edits: a new team, or this one. */
export interface TeamDialogData {
  readonly team?: Team;
}

/** Longest description, as the API checks it. */
const DESCRIPTION_MAX = 1000;

/** Creates or renames a team (L07-US02). Closes with the saved team. */
@Component({
  selector: 'app-team-dialog',
  imports: [MatButtonModule, MatDialogModule, MatFormFieldModule, MatInputModule, ReactiveFormsModule, TranslatableInput, TranslocoPipe],
  styleUrl: '../admin/provider-dialog.scss',
  template: `
    <h2 mat-dialog-title>
      @if (existing; as existing) {
        {{ 'teams.dialog.editTitle' | transloco: { name: existing.name } }}
      } @else {
        {{ 'teams.dialog.createTitle' | transloco }}
      }
    </h2>
    <form [formGroup]="form" (ngSubmit)="save()" novalidate>
      <mat-dialog-content class="fields">
        @if (errorKey(); as errorKey) {
          <p class="message error" role="alert">{{ errorKey | transloco }}</p>
        }
        <ui-translatable-input [group]="form.controls.nameTranslations" [label]="'teams.dialog.name' | transloco" />
        <mat-form-field appearance="outline">
          <mat-label>{{ 'teams.dialog.description' | transloco }}</mat-label>
          <textarea matInput formControlName="description" rows="3" [maxlength]="descriptionMax"></textarea>
          @if (form.controls.description.getError('api'); as key) {
            <mat-error>{{ key | transloco }}</mat-error>
          }
        </mat-form-field>
      </mat-dialog-content>
      <mat-dialog-actions align="end">
        <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
        <button matButton="filled" type="submit" [disabled]="saving()">
          {{ (existing ? 'teams.dialog.save' : 'teams.dialog.create') | transloco }}
        </button>
      </mat-dialog-actions>
    </form>
  `,
})
export class TeamDialog {
  protected readonly existing = inject<TeamDialogData>(MAT_DIALOG_DATA).team;
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<TeamDialog, Team>>(MatDialogRef);
  private readonly idempotencyKey = crypto.randomUUID();
  protected readonly descriptionMax = DESCRIPTION_MAX;

  protected readonly form = new FormGroup({
    nameTranslations: translationsForm(this.existing?.nameTranslations ?? {}),
    description: new FormControl(this.existing?.description ?? '', { nonNullable: true }),
  });
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  protected async save(): Promise<void> {
    for (const control of Object.values(this.form.controls.nameTranslations.controls)) control.updateValueAndValidity();
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    const body = {
      nameTranslations: translationsValue(this.form.controls.nameTranslations),
      description: this.form.controls.description.value.trim() || null,
    };
    try {
      const saved = this.existing
        ? await this.api.invoke(updateTeam, { id: this.existing.id, 'If-Match': `"${this.existing.version}"`, body })
        : await this.api.invoke(createTeam, { 'Idempotency-Key': this.idempotencyKey, body });
      this.dialogRef.close(saved);
    } catch (error) {
      this.errorKey.set(placeApiErrors(error, this.form.controls));
    } finally {
      this.saving.set(false);
    }
  }
}
