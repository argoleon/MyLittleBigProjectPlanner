import { Component, inject, signal } from '@angular/core';
import { FormControl, FormGroup, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { provideNativeDateAdapter } from '@angular/material/core';
import { MatDatepickerModule } from '@angular/material/datepicker';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { MatSelectModule } from '@angular/material/select';
import { MatSlideToggleModule } from '@angular/material/slide-toggle';
import { TranslocoPipe } from '@jsverse/transloco';
import { Absence, AbsenceKind, Api, createAbsence, fromLocalDate, toLocalDate, updateAbsence } from 'ui';

import { placeApiErrors } from '../configuration/form-errors';

/** Kinds of absences, in the order offered. */
export const ABSENCE_KINDS: readonly AbsenceKind[] = ['leave', 'training', 'other'];

/** What the dialog edits: a new absence of a resource, or this one. */
export interface AbsenceDialogData {
  readonly resourceId: string;
  readonly absence?: Absence;
}

/**
 * Declares or changes an absence (L07-US05): its kind, first and last
 * days, whether it starts or ends at noon, and a comment only the person
 * and the managers read. Closes with the saved absence.
 */
@Component({
  selector: 'app-absence-dialog',
  imports: [
    MatButtonModule,
    MatDatepickerModule,
    MatDialogModule,
    MatFormFieldModule,
    MatInputModule,
    MatSelectModule,
    MatSlideToggleModule,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  providers: [provideNativeDateAdapter()],
  styleUrl: '../admin/provider-dialog.scss',
  templateUrl: './absence-dialog.html',
})
export class AbsenceDialog {
  protected readonly data = inject<AbsenceDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<AbsenceDialog, Absence>>(MatDialogRef);
  private readonly idempotencyKey = crypto.randomUUID();
  protected readonly kinds = ABSENCE_KINDS;

  protected readonly form = new FormGroup({
    kind: new FormControl<AbsenceKind>(this.data.absence?.kind ?? 'leave', { nonNullable: true }),
    start: new FormControl<Date | null>(fromLocalDate(this.data.absence?.start), Validators.required),
    startsAtNoon: new FormControl(this.data.absence?.startsAtNoon ?? false, { nonNullable: true }),
    end: new FormControl<Date | null>(fromLocalDate(this.data.absence?.end), Validators.required),
    endsAtNoon: new FormControl(this.data.absence?.endsAtNoon ?? false, { nonNullable: true }),
    comment: new FormControl(this.data.absence?.comment ?? '', { nonNullable: true }),
  });
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  constructor() {
    // A one-day absence is the default: the last day follows the first.
    this.form.controls.start.valueChanges.subscribe((start) => {
      const end = this.form.controls.end;
      if (start && (!end.value || end.value < start)) end.setValue(start);
    });
  }

  protected async save(): Promise<void> {
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    const { kind, start, startsAtNoon, end, endsAtNoon, comment } = this.form.getRawValue();
    if (!start || !end) return;
    this.saving.set(true);
    this.errorKey.set(null);
    const body = {
      kind,
      start: toLocalDate(start),
      startsAtNoon,
      end: toLocalDate(end),
      endsAtNoon,
      comment: comment.trim() || null,
    };
    try {
      const existing = this.data.absence;
      const saved = existing
        ? await this.api.invoke(updateAbsence, { id: this.data.resourceId, absenceId: existing.id, 'If-Match': `"${existing.version}"`, body })
        : await this.api.invoke(createAbsence, { id: this.data.resourceId, 'Idempotency-Key': this.idempotencyKey, body });
      this.dialogRef.close(saved);
    } catch (error) {
      this.errorKey.set(placeApiErrors(error, this.form.controls));
    } finally {
      this.saving.set(false);
    }
  }
}
