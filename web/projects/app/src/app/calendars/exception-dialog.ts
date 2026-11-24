import { Component, inject, signal } from '@angular/core';
import { FormControl, FormGroup, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatButtonToggleModule } from '@angular/material/button-toggle';
import { provideNativeDateAdapter } from '@angular/material/core';
import { MatDatepickerModule } from '@angular/material/datepicker';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  CalendarException,
  CalendarExceptionKind,
  createCalendarException,
  createResourceException,
  fromLocalDate,
  toLocalDate,
  updateCalendarException,
  updateResourceException,
} from 'ui';

import { placeApiErrors } from '../configuration/form-errors';

/** Whose exception it is. */
export type ExceptionOwner = { readonly calendarId: string } | { readonly resourceId: string };

/** What the dialog edits: a new exception of `owner`, or this one. */
export interface ExceptionDialogData {
  readonly owner: ExceptionOwner;
  readonly exception?: CalendarException;
  /** The day a new exception starts on (`YYYY-MM-DD`). */
  readonly day?: string;
}

/** Kinds of exceptions, in the order offered. */
export const EXCEPTION_KINDS: readonly CalendarExceptionKind[] = ['non_working', 'holiday', 'working'];

/**
 * Adds or changes a dated exception of a calendar or a resource (L07-US04):
 * a closure, a holiday, or a day worked anyway. Closes with the saved one.
 */
@Component({
  selector: 'app-exception-dialog',
  imports: [
    MatButtonModule,
    MatButtonToggleModule,
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
    <h2 mat-dialog-title>{{ (data.exception ? 'calendars.exception.editTitle' : 'calendars.exception.createTitle') | transloco }}</h2>
    <form [formGroup]="form" (ngSubmit)="save()" novalidate>
      <mat-dialog-content class="fields">
        @if (errorKey(); as errorKey) {
          <p class="message error" role="alert">{{ errorKey | transloco }}</p>
        }
        <mat-form-field appearance="outline">
          <mat-label>{{ 'calendars.exception.day' | transloco }}</mat-label>
          <input matInput [matDatepicker]="picker" formControlName="day" required />
          <mat-datepicker-toggle matIconSuffix [for]="picker" />
          <mat-datepicker #picker />
          <mat-error>{{ (form.controls.day.getError('api') ?? 'errors.field.required') | transloco }}</mat-error>
        </mat-form-field>
        <div class="choice">
          <span class="choice-label" id="exception-kind-label">{{ 'calendars.exception.kind' | transloco }}</span>
          <mat-button-toggle-group formControlName="kind" aria-labelledby="exception-kind-label" hideSingleSelectionIndicator>
            @for (kind of kinds; track kind) {
              <mat-button-toggle [value]="kind">{{ 'calendars.kinds.' + kind | transloco }}</mat-button-toggle>
            }
          </mat-button-toggle-group>
          <p class="choice-hint">{{ 'calendars.kindHints.' + form.controls.kind.value | transloco }}</p>
        </div>
        <mat-form-field appearance="outline">
          <mat-label>{{ 'calendars.exception.label' | transloco }}</mat-label>
          <input matInput formControlName="label" maxlength="100" />
          @if (form.controls.label.getError('api'); as key) {
            <mat-error>{{ key | transloco }}</mat-error>
          }
        </mat-form-field>
      </mat-dialog-content>
      <mat-dialog-actions align="end">
        <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
        <button matButton="filled" type="submit" [disabled]="saving()">
          {{ (data.exception ? 'calendars.exception.save' : 'calendars.exception.create') | transloco }}
        </button>
      </mat-dialog-actions>
    </form>
  `,
})
export class ExceptionDialog {
  protected readonly data = inject<ExceptionDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<ExceptionDialog, CalendarException>>(MatDialogRef);
  private readonly idempotencyKey = crypto.randomUUID();
  protected readonly kinds = EXCEPTION_KINDS;

  protected readonly form = new FormGroup({
    day: new FormControl<Date | null>(fromLocalDate(this.data.exception?.day ?? this.data.day), Validators.required),
    kind: new FormControl<CalendarExceptionKind>(this.data.exception?.kind ?? 'non_working', { nonNullable: true }),
    label: new FormControl(this.data.exception?.label ?? '', { nonNullable: true }),
  });
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  protected async save(): Promise<void> {
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    const { day, kind, label } = this.form.getRawValue();
    if (!day) return;
    this.saving.set(true);
    this.errorKey.set(null);
    const body = { day: toLocalDate(day), kind, label: label.trim() || null };
    try {
      this.dialogRef.close(await this.send(body));
    } catch (error) {
      this.errorKey.set(placeApiErrors(error, this.form.controls));
    } finally {
      this.saving.set(false);
    }
  }

  private send(body: { day: string; kind: CalendarExceptionKind; label: string | null }): Promise<CalendarException> {
    const owner = this.data.owner;
    const existing = this.data.exception;
    if ('calendarId' in owner) {
      return existing
        ? this.api.invoke(updateCalendarException, { id: owner.calendarId, exceptionId: existing.id, 'If-Match': `"${existing.version}"`, body })
        : this.api.invoke(createCalendarException, { id: owner.calendarId, 'Idempotency-Key': this.idempotencyKey, body });
    }
    return existing
      ? this.api.invoke(updateResourceException, { id: owner.resourceId, exceptionId: existing.id, 'If-Match': `"${existing.version}"`, body })
      : this.api.invoke(createResourceException, { id: owner.resourceId, 'Idempotency-Key': this.idempotencyKey, body });
  }
}
