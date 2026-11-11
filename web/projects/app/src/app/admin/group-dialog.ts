import { Component, inject, signal } from '@angular/core';
import { FormBuilder, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  Group,
  createGroup,
  errorTranslationKey,
  fieldErrorTranslationKey,
  isApiProblem,
  updateGroup,
} from 'ui';

/** Creates a group, or renames one (`data`). Closes with the group saved. */
@Component({
  selector: 'app-group-dialog',
  imports: [MatButtonModule, MatDialogModule, MatFormFieldModule, MatInputModule, ReactiveFormsModule, TranslocoPipe],
  styleUrl: './provider-dialog.scss',
  template: `
    <h2 mat-dialog-title>{{ (group ? 'admin.groups.editTitle' : 'admin.groups.createTitle') | transloco }}</h2>
    <form [formGroup]="form" (ngSubmit)="save()" novalidate>
      <mat-dialog-content class="fields">
        @if (errorKey(); as key) {
          <p class="message error" role="alert">{{ key | transloco }}</p>
        }
        <mat-form-field appearance="outline">
          <mat-label>{{ 'admin.groups.name' | transloco }}</mat-label>
          <input matInput formControlName="name" maxlength="100" required />
          @if (form.controls.name.getError('api'); as key) {
            <mat-error>{{ key | transloco }}</mat-error>
          } @else {
            <mat-error>{{ 'errors.field.required' | transloco }}</mat-error>
          }
        </mat-form-field>
        <mat-form-field appearance="outline">
          <mat-label>{{ 'admin.groups.description' | transloco }}</mat-label>
          <textarea matInput formControlName="description" rows="3" maxlength="1000"></textarea>
        </mat-form-field>
      </mat-dialog-content>
      <mat-dialog-actions align="end">
        <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
        <button matButton="filled" type="submit" [disabled]="saving()">{{ 'admin.groups.save' | transloco }}</button>
      </mat-dialog-actions>
    </form>
  `,
})
export class GroupDialog {
  protected readonly group = inject<Group | undefined>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<GroupDialog, Group>>(MatDialogRef);

  protected readonly form = inject(FormBuilder).nonNullable.group({
    name: [this.group?.name ?? '', Validators.required],
    description: [this.group?.description ?? ''],
  });
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  private readonly idempotencyKey = crypto.randomUUID();

  protected async save(): Promise<void> {
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    const { name, description } = this.form.getRawValue();
    const body = { name: name.trim(), description: description.trim() || null };
    try {
      const saved = this.group
        ? await this.api.invoke(updateGroup, { id: this.group.id, 'If-Match': `"${this.group.version}"`, body })
        : await this.api.invoke(createGroup, { 'Idempotency-Key': this.idempotencyKey, body });
      this.dialogRef.close(saved);
    } catch (error) {
      const nameError = isApiProblem(error) ? error.fieldErrors.find((e) => e.field === 'name') : undefined;
      if (nameError) {
        this.form.controls.name.setErrors({ api: fieldErrorTranslationKey(nameError.code) });
        this.form.controls.name.markAsTouched();
      } else {
        this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
      }
    } finally {
      this.saving.set(false);
    }
  }
}
