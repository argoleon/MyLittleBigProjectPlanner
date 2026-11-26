import { Component, computed, inject, resource, signal } from '@angular/core';
import { toObservable, toSignal } from '@angular/core/rxjs-interop';
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
import { MatSlideToggleModule } from '@angular/material/slide-toggle';
import { TranslocoPipe, TranslocoService } from '@jsverse/transloco';
import { debounceTime, filter, map, merge } from 'rxjs';
import {
  Api,
  Assignment,
  AssignmentMode,
  AssignmentPreviewFields,
  AssignmentSegmentFields,
  DurationPipe,
  Issue,
  LocalDatePipe,
  LocaleService,
  OverloadDay,
  Resource,
  createIssueAssignment,
  errorTranslationKey,
  fieldErrorTranslationKey,
  fromLocalDate,
  isApiProblem,
  listResources,
  parseHours,
  previewAssignment,
  toLocalDate,
  updateAssignment,
} from 'ui';

import { hoursText } from '../capacity/week-hours';
import { overloadDaysText } from './overload-text';

/** Modes of a segment, in the order offered. */
export const MODES: readonly AssignmentMode[] = ['percentage', 'hours_per_day', 'total_effort'];

/** Largest total effort, in minutes: 9,999 hours. */
export const MAX_EFFORT_MINUTES = 9_999 * 60;

/** Most segments of an assignment. */
export const MAX_SEGMENTS = 50;

/** Quiet time after a change before the load preview is asked for, in ms. */
export const PREVIEW_DEBOUNCE_MS = 400;

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

/** Why the last save was refused for overload. */
interface Refusal {
  readonly days: readonly OverloadDay[];
  readonly forceable: boolean;
}

/**
 * Assigns a resource to a ticket, or changes an assignment (mockup 21,
 * L08-US01): the resource, found by name or job title, then one or more
 * segments, each with its period, its mode (percentage of the day's
 * capacity, hours per working day, or total effort over the period) and
 * its value. Field errors of the API land on the fields they name.
 *
 * While it is edited, a preview (L08-US03) shows for each day the
 * resource's existing load, this assignment's and the total, overloaded
 * days marked in text and color. An overload in blocking mode keeps it
 * from being saved, unless the member may force it ("Force overload",
 * traced); in warning mode it is saved and the days flagged. Closes with
 * the saved assignment.
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
    MatSlideToggleModule,
    ReactiveFormsModule,
    TranslocoPipe,
    DurationPipe,
  ],
  providers: [provideNativeDateAdapter()],
  templateUrl: './assignment-dialog.html',
  styleUrl: './assignment-dialog.scss',
})
export class AssignmentDialog {
  protected readonly data = inject<AssignmentDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<AssignmentDialog, Assignment>>(MatDialogRef);
  private readonly locale = inject(LocaleService);
  private readonly transloco = inject(TranslocoService);
  private readonly idempotencyKey = crypto.randomUUID();
  protected readonly modes = MODES;
  protected readonly maxSegments = MAX_SEGMENTS;

  protected readonly resourceInput = new FormControl<string | Resource>('', { nonNullable: true });
  protected readonly provisional = new FormControl(this.data.assignment?.provisional ?? false, { nonNullable: true });
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
  /** The last save refused for overload; any change clears it. */
  protected readonly refusal = signal<Refusal | null>(null);

  /** What the preview is asked for, once the form is quiet; none while incomplete. */
  private readonly previewFields = toSignal(
    merge(this.segments.valueChanges, this.provisional.valueChanges, toObservable(this.chosen)).pipe(
      debounceTime(PREVIEW_DEBOUNCE_MS),
      map(() => this.previewBody()),
    ),
    { initialValue: this.previewBody(), equal: (a, b) => JSON.stringify(a) === JSON.stringify(b) },
  );
  protected readonly preview = resource({
    params: () => this.previewFields() ?? undefined,
    loader: ({ params }) => this.api.invoke(previewAssignment, { body: params }),
  });
  /** The name of the resource previewed. */
  protected readonly resourceName = computed(() => this.data.assignment?.resource.displayName ?? this.chosen()?.displayName ?? '');
  /** The overload to tell: the save refused, else the preview's. */
  protected readonly overload = computed<{ days: readonly OverloadDay[]; blocked: boolean; forceable: boolean } | null>(() => {
    const refusal = this.refusal();
    if (refusal) return { days: refusal.days, blocked: true, forceable: refusal.forceable };
    if (!this.preview.hasValue()) return null;
    const preview = this.preview.value();
    return preview.overloads.length > 0 ? { days: preview.overloads, blocked: preview.blocked, forceable: preview.forceable } : null;
  });
  protected readonly overloadText = computed(() => {
    const overload = this.overload();
    if (!overload) return '';
    return overloadDaysText(overload.days, this.locale.formattingLocale(), (key, params) => this.transloco.translate(key, params));
  });

  constructor() {
    // A change answers the refusal: the preview tells again.
    merge(this.segments.valueChanges, this.provisional.valueChanges).subscribe(() => this.refusal.set(null));
  }

  protected readonly nameOf = (value: string | Resource | null): string =>
    typeof value === 'string' ? value : (value?.displayName ?? '');

  protected choose(event: MatAutocompleteSelectedEvent): void {
    this.chosen.set(event.option.value as Resource);
    this.resourceInput.setErrors(null);
  }

  /** Typing again drops the resource chosen. */
  protected typed(): void {
    this.chosen.set(null);
    this.refusal.set(null);
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
    void this.save(false);
  }

  /** Saves beyond capacity: members with "Force overload" only; traced. */
  protected force(): void {
    void this.save(true);
  }

  /** The preview's request; `null` while the resource or a segment is missing. */
  private previewBody(): AssignmentPreviewFields | null {
    const existing = this.data.assignment;
    const resourceId = existing?.resource.id ?? this.chosen()?.id;
    const segments = this.body();
    if (!resourceId || !segments || segments.length === 0) return null;
    return {
      issueId: this.data.issue.id,
      resourceId,
      assignmentId: existing?.id,
      provisional: this.provisional.value,
      segments,
    };
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

  private async save(force: boolean): Promise<void> {
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
    this.refusal.set(null);
    const provisional = this.provisional.value;
    try {
      const saved = existing
        ? await this.api.invoke(updateAssignment, {
            id: existing.id,
            'If-Match': `"${existing.version}"`,
            body: { provisional, segments, force },
          })
        : await this.api.invoke(createIssueAssignment, {
            idOrKey: this.data.issue.id,
            'Idempotency-Key': this.idempotencyKey,
            body: { resourceId: chosen?.id ?? '', provisional, segments, force },
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
    // Only an overload refusal carries its days.
    if (error.overload) {
      this.refusal.set({ days: error.overload.days, forceable: error.overload.forceable });
      return null;
    }
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
