import { Component, computed, inject, resource, signal } from '@angular/core';
import { toSignal } from '@angular/core/rxjs-interop';
import { AbstractControl, FormArray, FormControl, FormGroup, ReactiveFormsModule, ValidationErrors, Validators } from '@angular/forms';
import { MatAutocompleteModule, MatAutocompleteSelectedEvent } from '@angular/material/autocomplete';
import { MatButtonModule } from '@angular/material/button';
import { MatButtonToggleModule } from '@angular/material/button-toggle';
import { provideNativeDateAdapter } from '@angular/material/core';
import { MatDatepickerModule } from '@angular/material/datepicker';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { TranslocoPipe } from '@jsverse/transloco';
import { debounceTime, filter, map } from 'rxjs';
import {
  Api,
  Assignment,
  AssignmentMode,
  AssignmentSegmentFields,
  Issue,
  LocalDatePipe,
  Resource,
  createIssueAssignment,
  errorTranslationKey,
  fieldErrorTranslationKey,
  fromLocalDate,
  isApiProblem,
  listResources,
  parseHours,
  toLocalDate,
  updateAssignment,
} from 'ui';

import { hoursText } from '../capacity/week-hours';

/** Modes of a segment, in the order offered. */
export const MODES: readonly AssignmentMode[] = ['percentage', 'hours_per_day', 'total_effort'];

/** Largest total effort, in minutes: 9,999 hours. */
export const MAX_EFFORT_MINUTES = 9_999 * 60;

/** Most segments of an assignment. */
export const MAX_SEGMENTS = 50;

/** What the dialog edits: a new assignment on a ticket, or this one. */
export interface AssignmentDialogData {
  readonly issue: Issue;
  readonly assignment?: Assignment;
}

/** The value typed for `mode`: a percentage or hours; `null` if it is not one. */
export function segmentValue(mode: AssignmentMode, text: string): number | null {
  const value = text.trim().replace(/\s*%$/, '');
  if (mode === 'percentage') {
    const percent = /^\d{1,3}$/.test(value) ? Number(value) : NaN;
    return percent >= 1 && percent <= 100 ? percent : null;
  }
  const minutes = parseHours(value, mode === 'hours_per_day' ? 24 * 60 : MAX_EFFORT_MINUTES);
  return minutes && minutes > 0 ? minutes : null;
}

/** The text of a stored value for its field. */
export function valueText(mode: AssignmentMode, value: number): string {
  return mode === 'percentage' ? String(value) : hoursText(value);
}

function valueValidator(control: AbstractControl<string>): ValidationErrors | null {
  const mode = control.parent?.get('mode')?.value as AssignmentMode | undefined;
  if (!mode || (control.value ?? '').trim() === '') return null;
  return segmentValue(mode, control.value) === null ? { value: true } : null;
}

type SegmentForm = FormGroup<{
  start: FormControl<Date | null>;
  end: FormControl<Date | null>;
  mode: FormControl<AssignmentMode>;
  value: FormControl<string>;
}>;

function segmentForm(start: string | null | undefined, end: string | null | undefined, mode: AssignmentMode, value: string): SegmentForm {
  const form: SegmentForm = new FormGroup({
    start: new FormControl<Date | null>(fromLocalDate(start), Validators.required),
    end: new FormControl<Date | null>(fromLocalDate(end), Validators.required),
    mode: new FormControl<AssignmentMode>(mode, { nonNullable: true }),
    value: new FormControl(value, { nonNullable: true, validators: [Validators.required, valueValidator] }),
  });
  // The value reads differently in each mode.
  form.controls.mode.valueChanges.subscribe(() => form.controls.value.updateValueAndValidity());
  return form;
}

/**
 * The control a field error of the API names (`resourceId`,
 * `segments.1.end`…), in `segments` and `resource`; `null` when none holds
 * it (`segments` as a whole, a segment as a whole is its period).
 */
export function controlOf(field: string, segments: FormArray<SegmentForm>, resourceControl: AbstractControl): AbstractControl | null {
  if (field === 'resourceId') return resourceControl;
  const match = /^segments\.(\d+)(?:\.(start|end|value))?$/.exec(field);
  const segment = match ? segments.at(Number(match[1])) : undefined;
  if (!match || !segment) return null;
  return segment.controls[(match[2] as 'start' | 'end' | 'value' | undefined) ?? 'start'];
}

/** Resources offered at once in the search. */
const CHOICES = 20;

/**
 * Assigns a resource to a ticket, or changes an assignment (mockup 21,
 * L08-US01): the resource, found by name or job title, then one or more
 * segments, each with its period, its mode (percentage of the day's
 * capacity, hours per working day, or total effort over the period) and
 * its value. Field errors of the API land on the fields they name.
 * Closes with the saved assignment.
 */
@Component({
  selector: 'app-assignment-dialog',
  imports: [
    LocalDatePipe,
    MatAutocompleteModule,
    MatButtonModule,
    MatButtonToggleModule,
    MatDatepickerModule,
    MatDialogModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  providers: [provideNativeDateAdapter()],
  templateUrl: './assignment-dialog.html',
  styleUrl: './assignment-dialog.scss',
})
export class AssignmentDialog {
  protected readonly data = inject<AssignmentDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<AssignmentDialog, Assignment>>(MatDialogRef);
  private readonly idempotencyKey = crypto.randomUUID();
  protected readonly modes = MODES;
  protected readonly maxSegments = MAX_SEGMENTS;

  protected readonly resourceInput = new FormControl<string | Resource>('', { nonNullable: true });
  protected readonly segments = new FormArray<SegmentForm>(
    this.data.assignment
      ? this.data.assignment.segments.map((s) => segmentForm(s.start, s.end, s.mode, valueText(s.mode, s.value)))
      : [segmentForm(this.data.issue.startDate, this.data.issue.dueDate, 'hours_per_day', '')],
  );
  private readonly search = toSignal(
    this.resourceInput.valueChanges.pipe(
      // Choosing a resource puts it in the field: only typed text searches.
      filter((value): value is string => typeof value === 'string'),
      debounceTime(250),
      map((value) => value.trim()),
    ),
    { initialValue: '' },
  );
  private readonly found = resource({
    params: () => (this.data.assignment ? undefined : { search: this.search() }),
    loader: ({ params }) => this.api.invoke(listResources, { search: params.search || undefined, active: true, limit: CHOICES }),
  });
  protected readonly choices = computed<readonly Resource[]>(() => (this.found.hasValue() ? this.found.value().items : []));
  protected readonly chosen = signal<Resource | null>(null);
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  protected readonly nameOf = (value: string | Resource | null): string =>
    typeof value === 'string' ? value : (value?.displayName ?? '');

  protected choose(event: MatAutocompleteSelectedEvent): void {
    this.chosen.set(event.option.value as Resource);
    this.resourceInput.setErrors(null);
  }

  /** Typing again drops the resource chosen. */
  protected typed(): void {
    this.chosen.set(null);
  }

  /** A new segment, from the day after the last one, in its mode. */
  protected addSegment(): void {
    const last = this.segments.at(this.segments.length - 1)?.getRawValue();
    const start = last?.end ? toLocalDate(new Date(last.end.getFullYear(), last.end.getMonth(), last.end.getDate() + 1)) : this.data.issue.startDate;
    this.segments.push(segmentForm(start, this.data.issue.dueDate, last?.mode ?? 'hours_per_day', ''));
  }

  protected removeSegment(index: number): void {
    this.segments.removeAt(index);
  }

  protected suffixKey(mode: AssignmentMode): string {
    return `assignments.dialog.suffix.${mode}`;
  }

  /** A plain form: its submission stays in the page. */
  protected submit(event: Event): void {
    event.preventDefault();
    void this.save();
  }

  private body(): AssignmentSegmentFields[] | null {
    const segments: AssignmentSegmentFields[] = [];
    for (const form of this.segments.controls) {
      const { start, end, mode, value } = form.getRawValue();
      const parsed = segmentValue(mode, value);
      if (!start || !end || parsed === null) return null;
      segments.push({ start: toLocalDate(start), end: toLocalDate(end), mode, value: parsed });
    }
    return segments;
  }

  private async save(): Promise<void> {
    const existing = this.data.assignment;
    const chosen = this.chosen();
    if (!existing && !chosen) {
      this.resourceInput.setErrors({ api: 'errors.field.required' });
      this.resourceInput.markAsTouched();
    }
    const segments = this.body();
    if (this.segments.invalid || !segments || (!existing && !chosen)) {
      this.segments.markAllAsTouched();
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    try {
      const saved = existing
        ? await this.api.invoke(updateAssignment, {
            id: existing.id,
            'If-Match': `"${existing.version}"`,
            body: { provisional: existing.provisional, segments },
          })
        : await this.api.invoke(createIssueAssignment, {
            idOrKey: this.data.issue.id,
            'Idempotency-Key': this.idempotencyKey,
            body: { resourceId: chosen?.id ?? '', segments },
          });
      this.dialogRef.close(saved);
    } catch (error) {
      this.errorKey.set(this.placeErrors(error));
    } finally {
      this.saving.set(false);
    }
  }

  /** Puts field errors on their fields; the key to show above the form, if any. */
  private placeErrors(error: unknown): string | null {
    if (!isApiProblem(error)) return errorTranslationKey('unknown');
    let left = error.fieldErrors.length === 0 || error.code !== 'validation_failed';
    for (const fieldError of error.fieldErrors) {
      const control = controlOf(fieldError.field, this.segments, this.resourceInput);
      if (control) {
        control.setErrors({ api: fieldErrorTranslationKey(fieldError.code) });
        control.markAsTouched();
      } else {
        left = true;
      }
    }
    return left ? error.translationKey : null;
  }
}
