import { Component, computed, inject, input } from '@angular/core';
import { AbstractControl, FormArray, FormControl, ReactiveFormsModule, ValidationErrors } from '@angular/forms';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { TranslocoPipe } from '@jsverse/transloco';
import { DurationPipe, LocaleService, parseHours, weekdayNames } from 'ui';

/** Seven fields of hours, Monday first. */
export type WeekHoursForm = FormArray<FormControl<string>>;

/** How a number of minutes is typed back into a field: `7`, `3:30`. */
export function hoursText(minutes: number): string {
  const hours = Math.trunc(minutes / 60);
  const rest = minutes % 60;
  return rest === 0 ? String(hours) : `${hours}:${String(rest).padStart(2, '0')}`;
}

/** A field holds hours of a day: `7`, `3,5`, `3:30`. */
export function hoursValidator(control: AbstractControl<string>): ValidationErrors | null {
  return parseHours(control.value ?? '') === null ? { hours: true } : null;
}

/** The fields of a week of `minutes`, Monday first. */
export function weekHoursForm(minutes: readonly number[]): WeekHoursForm {
  return new FormArray(
    Array.from({ length: 7 }, (_, day) => new FormControl(hoursText(minutes[day] ?? 0), { nonNullable: true, validators: hoursValidator })),
  );
}

/** The minutes of each day of `form`, Monday first (an invalid field counts 0). */
export function weekHoursValue(form: WeekHoursForm): number[] {
  return form.controls.map((control) => parseHours(control.value) ?? 0);
}

/**
 * The hours of each day of a week (L07-US03), one field per day, Monday
 * first, with the week's total. Typed as `7`, `3,5`, `3.5` or `3:30`.
 */
@Component({
  selector: 'app-week-hours',
  imports: [DurationPipe, MatFormFieldModule, MatInputModule, ReactiveFormsModule, TranslocoPipe],
  template: `
    <fieldset class="week">
      <legend class="legend">{{ legend() }}</legend>
      <div class="days">
        @for (control of form().controls; track $index; let day = $index) {
          <mat-form-field appearance="outline" class="day" subscriptSizing="dynamic">
            <mat-label>{{ days()[day] }}</mat-label>
            <input matInput inputmode="decimal" [formControl]="control" [attr.aria-describedby]="hintId" />
            <span matTextSuffix>h</span>
            @if (control.hasError('api')) {
              <mat-error>{{ control.getError('api') | transloco }}</mat-error>
            } @else if (control.hasError('hours')) {
              <mat-error>{{ 'capacity.hoursInvalid' | transloco }}</mat-error>
            }
          </mat-form-field>
        }
      </div>
      <p class="hint" [id]="hintId">
        {{ 'capacity.hoursHint' | transloco }} ·
        {{ 'capacity.weekTotal' | transloco: { total: (total() | duration) } }}
      </p>
    </fieldset>
  `,
  styleUrl: './week-hours.scss',
})
export class WeekHours {
  /** The seven fields. */
  readonly form = input.required<WeekHoursForm>();
  /** What the fields are, read before them. */
  readonly legend = input.required<string>();

  private static next = 0;
  protected readonly hintId = `week-hours-hint-${WeekHours.next++}`;
  private readonly locale = inject(LocaleService);
  protected readonly days = computed(() => weekdayNames(this.locale.formattingLocale()));
  /** The week's minutes, from the fields as typed (template events refresh it). */
  protected total(): number {
    return weekHoursValue(this.form()).reduce((sum, minutes) => sum + minutes, 0);
  }
}

