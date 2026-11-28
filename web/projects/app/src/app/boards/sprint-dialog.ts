import { Component, inject, signal } from '@angular/core';
import { FormControl, FormGroup, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { provideNativeDateAdapter } from '@angular/material/core';
import { MatDatepickerModule } from '@angular/material/datepicker';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  Sprint,
  createSprint,
  errorTranslationKey,
  fieldErrorTranslationKey,
  fromLocalDate,
  isApiProblem,
  toLocalDate,
  updateSprint,
} from 'ui';

/** What the dialog edits: a new sprint of a Scrum board, or this one. */
export interface SprintDialogData {
  readonly boardId: string;
  readonly sprint?: Sprint;
}

/** Longest name and goal, as the API checks them. */
export const NAME_MAX = 100;
export const GOAL_MAX = 1000;

/**
 * Creates or changes a sprint of a Scrum board (mockup 17, L09-US01): its
 * name (left empty at creation, the API names it after the node's key),
 * goal and dates, both or none. Closes with the saved sprint.
 */
@Component({
  selector: 'app-sprint-dialog',
  imports: [
    MatButtonModule,
    MatDatepickerModule,
    MatDialogModule,
    MatFormFieldModule,
    MatInputModule,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  providers: [provideNativeDateAdapter()],
  styleUrl: '../admin/provider-dialog.scss',
  template: `
    <h2 mat-dialog-title>
      {{ (data.sprint ? 'backlog.dialog.editTitle' : 'backlog.dialog.newTitle') | transloco }}
    </h2>
    <form [formGroup]="form" (ngSubmit)="save()" novalidate>
      <mat-dialog-content class="fields">
        @if (errorKey(); as errorKey) {
          <p class="message error" role="alert">{{ errorKey | transloco }}</p>
        }
        <mat-form-field appearance="outline">
          <mat-label>{{ 'backlog.dialog.name' | transloco }}</mat-label>
          <input
            matInput
            formControlName="name"
            [maxlength]="nameMax"
            autocomplete="off"
            [required]="!!data.sprint"
          />
          @if (!data.sprint) {
            <mat-hint>{{ 'backlog.dialog.nameHint' | transloco }}</mat-hint>
          }
          <mat-error>{{
            form.controls.name.getError('api') ?? 'errors.field.required' | transloco
          }}</mat-error>
        </mat-form-field>
        <mat-form-field appearance="outline">
          <mat-label>{{ 'backlog.dialog.goal' | transloco }}</mat-label>
          <textarea matInput formControlName="goal" rows="2" [maxlength]="goalMax"></textarea>
          @if (form.controls.goal.getError('api'); as key) {
            <mat-error>{{ key | transloco }}</mat-error>
          }
        </mat-form-field>
        <mat-form-field appearance="outline">
          <mat-label>{{ 'backlog.dialog.dates' | transloco }}</mat-label>
          <mat-date-range-input [rangePicker]="picker" [formGroup]="form.controls.dates">
            <input
              matStartDate
              formControlName="start"
              [placeholder]="'backlog.dialog.start' | transloco"
            />
            <input
              matEndDate
              formControlName="end"
              [placeholder]="'backlog.dialog.end' | transloco"
            />
          </mat-date-range-input>
          <mat-datepicker-toggle matIconSuffix [for]="picker" />
          <mat-date-range-picker #picker />
          <mat-hint>{{ 'backlog.dialog.datesHint' | transloco }}</mat-hint>
        </mat-form-field>
        @if (datesError(); as key) {
          <p class="message error dates-error" role="alert">{{ key | transloco }}</p>
        }
      </mat-dialog-content>
      <mat-dialog-actions align="end">
        <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
        <button matButton="filled" type="submit" class="save" [disabled]="saving()">
          {{ (data.sprint ? 'backlog.dialog.save' : 'backlog.dialog.create') | transloco }}
        </button>
      </mat-dialog-actions>
    </form>
  `,
})
export class SprintDialog {
  protected readonly data = inject<SprintDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<SprintDialog, Sprint>>(MatDialogRef);
  protected readonly nameMax = NAME_MAX;
  protected readonly goalMax = GOAL_MAX;

  protected readonly form = new FormGroup({
    name: new FormControl(this.data.sprint?.name ?? '', {
      nonNullable: true,
      validators: this.data.sprint
        ? [Validators.required, Validators.maxLength(NAME_MAX)]
        : [Validators.maxLength(NAME_MAX)],
    }),
    goal: new FormControl(this.data.sprint?.goal ?? '', {
      nonNullable: true,
      validators: [Validators.maxLength(GOAL_MAX)],
    }),
    dates: new FormGroup({
      start: new FormControl<Date | null>(fromLocalDate(this.data.sprint?.startDate)),
      end: new FormControl<Date | null>(fromLocalDate(this.data.sprint?.endDate)),
    }),
  });
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  /** Why the dates were refused: one only, or the end before the start. */
  protected readonly datesError = signal<string | null>(null);

  protected async save(): Promise<void> {
    this.datesError.set(null);
    const { name, goal, dates } = this.form.getRawValue();
    if (!dates.start !== !dates.end) {
      this.datesError.set('backlog.dialog.bothDates');
      return;
    }
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    const body = {
      name: name.trim() || null,
      goal: goal.trim() || null,
      startDate: dates.start ? toLocalDate(dates.start) : null,
      endDate: dates.end ? toLocalDate(dates.end) : null,
    };
    try {
      const saved = this.data.sprint
        ? await this.api.invoke(updateSprint, {
            id: this.data.sprint.id,
            'If-Match': `"${this.data.sprint.version}"`,
            body,
          })
        : await this.api.invoke(createSprint, { id: this.data.boardId, body });
      this.dialogRef.close(saved);
    } catch (error) {
      this.showError(error);
    } finally {
      this.saving.set(false);
    }
  }

  /** Field errors on their field; anything else above the form. */
  private showError(error: unknown): void {
    if (!isApiProblem(error)) {
      this.errorKey.set(errorTranslationKey('unknown'));
      return;
    }
    if (error.status === 412) {
      this.errorKey.set('backlog.dialog.conflict');
      return;
    }
    let placed = false;
    for (const fieldError of error.fieldErrors) {
      const key = fieldErrorTranslationKey(fieldError.code);
      if (fieldError.field === 'name' || fieldError.field === 'goal') {
        const control = this.form.controls[fieldError.field];
        control.setErrors({ api: key });
        control.markAsTouched();
        placed = true;
      } else if (fieldError.field === 'startDate' || fieldError.field === 'endDate') {
        this.datesError.set(key);
        placed = true;
      }
    }
    if (!placed || error.code !== 'validation_failed') this.errorKey.set(error.translationKey);
  }
}
