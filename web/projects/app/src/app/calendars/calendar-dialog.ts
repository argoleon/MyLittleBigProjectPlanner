import { Component, inject, signal } from '@angular/core';
import { FormControl, FormGroup, ReactiveFormsModule } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatSelectModule } from '@angular/material/select';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, Calendar, HolidaySet, TranslatableInput, createCalendar, translationsForm, translationsValue, updateCalendar } from 'ui';

import { placeApiErrors } from '../configuration/form-errors';

/** The holiday sets offered, in order. */
export const HOLIDAY_SETS: readonly HolidaySet[] = ['fr', 'fr-alsace-moselle', 'none'];

/** What the dialog edits: a new site, or this calendar. */
export interface CalendarDialogData {
  readonly calendar?: Calendar;
}

/**
 * Creates a site calendar or changes a calendar (L07-US04): its name and
 * its public holidays. A site may use the organization's holidays; the
 * organization's calendar always has a set. Closes with the saved calendar.
 */
@Component({
  selector: 'app-calendar-dialog',
  imports: [MatButtonModule, MatDialogModule, MatFormFieldModule, MatSelectModule, ReactiveFormsModule, TranslatableInput, TranslocoPipe],
  styleUrl: '../admin/provider-dialog.scss',
  template: `
    <h2 mat-dialog-title>
      @if (existing; as existing) {
        {{ 'calendars.dialog.editTitle' | transloco: { name: existing.name } }}
      } @else {
        {{ 'calendars.dialog.createTitle' | transloco }}
      }
    </h2>
    <form [formGroup]="form" (ngSubmit)="save()" novalidate>
      <mat-dialog-content class="fields">
        @if (errorKey(); as errorKey) {
          <p class="message error" role="alert">{{ errorKey | transloco }}</p>
        }
        <ui-translatable-input [group]="form.controls.nameTranslations" [label]="'calendars.dialog.name' | transloco" />
        <mat-form-field appearance="outline">
          <mat-label>{{ 'calendars.dialog.holidays' | transloco }}</mat-label>
          <mat-select formControlName="holidaySet">
            @if (!isTenant) {
              <mat-option [value]="null">{{ 'calendars.sets.inherit' | transloco }}</mat-option>
            }
            @for (set of sets; track set) {
              <mat-option [value]="set">{{ 'calendars.sets.' + set | transloco }}</mat-option>
            }
          </mat-select>
          <mat-hint>{{ (isTenant ? 'calendars.dialog.tenantHint' : 'calendars.dialog.siteHint') | transloco }}</mat-hint>
        </mat-form-field>
      </mat-dialog-content>
      <mat-dialog-actions align="end">
        <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
        <button matButton="filled" type="submit" [disabled]="saving()">
          {{ (existing ? 'calendars.dialog.save' : 'calendars.dialog.create') | transloco }}
        </button>
      </mat-dialog-actions>
    </form>
  `,
})
export class CalendarDialog {
  protected readonly existing = inject<CalendarDialogData>(MAT_DIALOG_DATA).calendar;
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<CalendarDialog, Calendar>>(MatDialogRef);
  private readonly idempotencyKey = crypto.randomUUID();
  protected readonly sets = HOLIDAY_SETS;
  protected readonly isTenant = this.existing?.kind === 'tenant';

  protected readonly form = new FormGroup({
    nameTranslations: translationsForm(this.existing?.nameTranslations ?? {}),
    holidaySet: new FormControl<HolidaySet | null>(this.existing ? this.existing.holidaySet : null),
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
      holidaySet: this.form.controls.holidaySet.value,
    };
    try {
      const saved = this.existing
        ? await this.api.invoke(updateCalendar, { id: this.existing.id, 'If-Match': `"${this.existing.version}"`, body })
        : await this.api.invoke(createCalendar, { 'Idempotency-Key': this.idempotencyKey, body });
      this.dialogRef.close(saved);
    } catch (error) {
      this.errorKey.set(placeApiErrors(error, this.form.controls));
    } finally {
      this.saving.set(false);
    }
  }
}
