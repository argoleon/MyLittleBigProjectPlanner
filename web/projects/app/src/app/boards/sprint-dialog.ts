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
  startSprint,
  toLocalDate,
  updateSprint,
} from 'ui';

import { defaultDates } from './sprint-dates';

/**
 * What the dialog does: creates a sprint of a Scrum board, changes this one,
 * or starts it (`start`, L09-US02).
 */
export interface SprintDialogData {
  readonly boardId: string;
  readonly sprint?: Sprint;
  readonly start?: boolean;
  /** Today, as a local date: where a sprint without dates starts. */
  readonly today?: string;
}

/** Longest name and goal, as the API checks them. */
export const NAME_MAX = 100;
export const GOAL_MAX = 1000;

/**
 * Creates or changes a sprint of a Scrum board (mockup 17, L09-US01): its
 * name (left empty at creation, the API names it after the node's key),
 * goal and dates, both or none. Starting a sprint (L09-US02) takes the same
 * fields, the dates then required: two weeks from today unless it has its
 * own. Closes with the saved sprint.
 */
@Component({
  selector: 'app-sprint-dialog',
  imports: [MatButtonModule, MatDatepickerModule, MatDialogModule, MatFormFieldModule, MatInputModule, ReactiveFormsModule, TranslocoPipe],
  providers: [provideNativeDateAdapter()],
  styleUrl: '../admin/provider-dialog.scss',
  templateUrl: './sprint-dialog.html',
})
export class SprintDialog {
  protected readonly data = inject<SprintDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<SprintDialog, Sprint>>(MatDialogRef);
  protected readonly nameMax = NAME_MAX;
  protected readonly goalMax = GOAL_MAX;
  /** `create`, `edit` or `start`. */
  protected readonly mode = this.data.start ? 'start' : this.data.sprint ? 'edit' : 'create';

  private readonly dates = this.mode === 'start' && !this.data.sprint?.startDate ? defaultDates(this.data.today ?? toLocalDate(new Date())) : null;
  protected readonly form = new FormGroup({
    name: new FormControl(this.data.sprint?.name ?? '', {
      nonNullable: true,
      validators: this.data.sprint ? [Validators.required, Validators.maxLength(NAME_MAX)] : [Validators.maxLength(NAME_MAX)],
    }),
    goal: new FormControl(this.data.sprint?.goal ?? '', { nonNullable: true, validators: [Validators.maxLength(GOAL_MAX)] }),
    dates: new FormGroup({
      start: new FormControl<Date | null>(fromLocalDate(this.dates?.start ?? this.data.sprint?.startDate)),
      end: new FormControl<Date | null>(fromLocalDate(this.dates?.end ?? this.data.sprint?.endDate)),
    }),
  });
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  /** Why the dates were refused: one only, none to start, or the end before the start. */
  protected readonly datesError = signal<string | null>(null);

  protected async save(): Promise<void> {
    this.datesError.set(null);
    const { name, goal, dates } = this.form.getRawValue();
    if (this.mode === 'start' && (!dates.start || !dates.end)) {
      this.datesError.set('backlog.dialog.datesRequired');
      return;
    }
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
    const sprint = this.data.sprint;
    try {
      const saved = !sprint
        ? await this.api.invoke(createSprint, { id: this.data.boardId, body })
        : this.mode === 'start'
          ? await this.api.invoke(startSprint, { id: sprint.id, 'If-Match': `"${sprint.version}"`, body })
          : await this.api.invoke(updateSprint, { id: sprint.id, 'If-Match': `"${sprint.version}"`, body });
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
