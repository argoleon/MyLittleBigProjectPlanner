import { Component, inject, signal } from '@angular/core';
import { FormControl, FormGroup, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatDatepickerModule } from '@angular/material/datepicker';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { provideNativeDateAdapter } from '@angular/material/core';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  CapacityPattern,
  createCapacityPattern,
  errorTranslationKey,
  fieldErrorTranslationKey,
  fromLocalDate,
  isApiProblem,
  toLocalDate,
  updateCapacityPattern,
} from 'ui';

import { WeekHours, weekHoursForm, weekHoursValue } from './week-hours';

/** What the dialog edits: a new period of a resource, or this one. */
export interface PeriodDialogData {
  readonly resourceId: string;
  readonly period?: CapacityPattern;
  /** The week a new period starts from: the latest one, else the tenant's default. */
  readonly startingWeek: readonly number[];
}

/** Longest label, as the API checks it. */
const LABEL_MAX = 100;

/**
 * Adds or changes a capacity period of a resource (L07-US03): its first
 * day, hours per weekday and label. Closes with the saved period.
 */
@Component({
  selector: 'app-period-dialog',
  imports: [
    MatButtonModule,
    MatDatepickerModule,
    MatDialogModule,
    MatFormFieldModule,
    MatInputModule,
    ReactiveFormsModule,
    TranslocoPipe,
    WeekHours,
  ],
  providers: [provideNativeDateAdapter()],
  styleUrl: '../admin/provider-dialog.scss',
  template: `
    <h2 mat-dialog-title>{{ (data.period ? 'capacity.dialog.editTitle' : 'capacity.dialog.createTitle') | transloco }}</h2>
    <form [formGroup]="form" (ngSubmit)="save()" novalidate>
      <mat-dialog-content class="fields">
        @if (errorKey(); as errorKey) {
          <p class="message error" role="alert">{{ errorKey | transloco }}</p>
        }
        <mat-form-field appearance="outline">
          <mat-label>{{ 'capacity.dialog.from' | transloco }}</mat-label>
          <input matInput [matDatepicker]="picker" formControlName="effectiveFrom" required />
          <mat-datepicker-toggle matIconSuffix [for]="picker" />
          <mat-datepicker #picker />
          <mat-hint>{{ 'capacity.dialog.fromHint' | transloco }}</mat-hint>
          <mat-error>{{ (form.controls.effectiveFrom.getError('api') ?? 'errors.field.required') | transloco }}</mat-error>
        </mat-form-field>
        <app-week-hours [form]="form.controls.week" [legend]="'capacity.dialog.week' | transloco" />
        <mat-form-field appearance="outline">
          <mat-label>{{ 'capacity.dialog.label' | transloco }}</mat-label>
          <input matInput formControlName="label" [maxlength]="labelMax" />
          <mat-hint>{{ 'capacity.dialog.labelHint' | transloco }}</mat-hint>
          @if (form.controls.label.getError('api'); as key) {
            <mat-error>{{ key | transloco }}</mat-error>
          }
        </mat-form-field>
      </mat-dialog-content>
      <mat-dialog-actions align="end">
        <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
        <button matButton="filled" type="submit" [disabled]="saving()">
          {{ (data.period ? 'capacity.dialog.save' : 'capacity.dialog.create') | transloco }}
        </button>
      </mat-dialog-actions>
    </form>
  `,
})
export class PeriodDialog {
  protected readonly data = inject<PeriodDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<PeriodDialog, CapacityPattern>>(MatDialogRef);
  private readonly idempotencyKey = crypto.randomUUID();
  protected readonly labelMax = LABEL_MAX;

  protected readonly form = new FormGroup({
    effectiveFrom: new FormControl<Date | null>(fromLocalDate(this.data.period?.effectiveFrom), Validators.required),
    week: weekHoursForm(this.data.period?.minutes ?? this.data.startingWeek),
    label: new FormControl(this.data.period?.label ?? '', { nonNullable: true }),
  });
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  protected async save(): Promise<void> {
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    const { effectiveFrom, label } = this.form.getRawValue();
    if (!effectiveFrom) return;
    this.saving.set(true);
    this.errorKey.set(null);
    const body = {
      effectiveFrom: toLocalDate(effectiveFrom),
      minutes: weekHoursValue(this.form.controls.week),
      label: label.trim() || null,
    };
    try {
      const saved = this.data.period
        ? await this.api.invoke(updateCapacityPattern, {
            id: this.data.resourceId,
            patternId: this.data.period.id,
            'If-Match': `"${this.data.period.version}"`,
            body,
          })
        : await this.api.invoke(createCapacityPattern, { id: this.data.resourceId, 'Idempotency-Key': this.idempotencyKey, body });
      this.dialogRef.close(saved);
    } catch (error) {
      this.showError(error);
    } finally {
      this.saving.set(false);
    }
  }

  /** Field errors on their field; a conflict says more than its field. */
  private showError(error: unknown): void {
    if (!isApiProblem(error)) {
      this.errorKey.set(errorTranslationKey('unknown'));
      return;
    }
    let placed = false;
    for (const fieldError of error.fieldErrors) {
      const day = /^minutes\.(\d)$/.exec(fieldError.field);
      const control =
        fieldError.field === 'effectiveFrom'
          ? this.form.controls.effectiveFrom
          : fieldError.field === 'label'
            ? this.form.controls.label
            : day
              ? this.form.controls.week.at(Number(day[1]))
              : null;
      if (control) {
        control.setErrors({ api: fieldErrorTranslationKey(fieldError.code) });
        control.markAsTouched();
        placed = true;
      }
    }
    if (!placed || error.code !== 'validation_failed') this.errorKey.set(error.translationKey);
  }
}
