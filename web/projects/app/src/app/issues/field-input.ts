import { COMMA, ENTER, SPACE } from '@angular/cdk/keycodes';
import { Component, computed, input } from '@angular/core';
import { FormControl, ReactiveFormsModule } from '@angular/forms';
import { MatCheckboxModule } from '@angular/material/checkbox';
import { MatChipInputEvent, MatChipsModule } from '@angular/material/chips';
import { MatDatepickerModule } from '@angular/material/datepicker';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { MatSelectModule } from '@angular/material/select';
import { TranslocoPipe } from '@jsverse/transloco';
import { IssueFormField, IssueFormValue } from 'ui';

import { FieldRaw } from './issue-fields';
import { PeoplePicker } from './people-picker';

/** The translation key of the first error of `control`, if shown. */
export function errorKeyOf(control: FormControl<unknown>): string | null {
  if (!control.touched || !control.errors) return null;
  const errors = control.errors;
  if (errors['api']) return errors['api'] as string;
  if (errors['required']) return 'errors.field.required';
  if (errors['maxlength']) return 'errors.field.too_long';
  if (errors['min'] || errors['max']) return 'errors.field.out_of_range';
  if (errors['before_start']) return 'errors.field.before_start';
  return 'errors.field.format';
}

/**
 * One custom field of the creation screen, by its kind: text, number,
 * checkbox, dates, lists, people, groups, labels, web address or ticket.
 */
@Component({
  selector: 'app-field-input',
  imports: [
    MatCheckboxModule,
    MatChipsModule,
    MatDatepickerModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    MatSelectModule,
    PeoplePicker,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  templateUrl: './field-input.html',
  styleUrl: './field-input.scss',
})
export class FieldInput {
  readonly field = input.required<IssueFormField>();
  readonly control = input.required<FormControl<FieldRaw>>();
  readonly projectKey = input.required<string>();

  protected readonly separators = [ENTER, COMMA, SPACE] as const;
  protected readonly label = computed(() => this.field().label ?? this.field().key);
  protected readonly required = computed(() => this.field().mode === 'required');
  protected readonly values = computed<readonly IssueFormValue[]>(() => this.field().values ?? []);
  protected readonly step = computed(() => {
    const decimals = this.field().decimals;
    return decimals == null ? 'any' : decimals === 0 ? '1' : `0.${'0'.repeat(decimals - 1)}1`;
  });

  protected errorKey(): string | null {
    return errorKeyOf(this.control());
  }

  protected list(): readonly string[] {
    const value = this.control().value;
    return Array.isArray(value) ? (value as readonly string[]) : [];
  }

  /** The children of the value chosen first, for a cascading list. */
  protected children(): readonly IssueFormValue[] {
    const [parent] = this.list();
    return this.values().find((v) => v.id === parent)?.children ?? [];
  }

  protected setCascade(level: 0 | 1, id: string | null): void {
    const [parent] = this.list();
    const value = level === 0 ? (id ? [id] : []) : parent ? (id ? [parent, id] : [parent]) : [];
    this.control().setValue(value);
    this.control().markAsDirty();
    this.control().markAsTouched();
  }

  protected addLabel(event: MatChipInputEvent): void {
    const label = event.value.trim();
    if (label && !this.list().includes(label)) {
      this.control().setValue([...this.list(), label]);
      this.control().markAsDirty();
    }
    event.chipInput.clear();
  }

  protected removeLabel(label: string): void {
    this.control().setValue(this.list().filter((l) => l !== label));
    this.control().markAsDirty();
    this.control().markAsTouched();
  }
}
