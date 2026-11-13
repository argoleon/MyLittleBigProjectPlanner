import { Component, effect, input, output, untracked } from '@angular/core';
import { AbstractControl, FormControl, ReactiveFormsModule, ValidationErrors } from '@angular/forms';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { TranslocoPipe } from '@jsverse/transloco';

import { parseWeight } from './node-links';

function weightValidator(control: AbstractControl<string>): ValidationErrors | null {
  return parseWeight(control.value) === null ? { out_of_range: true } : null;
}

/**
 * Weighting of a parent link, 0 to 100 %: emits a valid new value when the
 * field is left or Enter is pressed (the input's `change`).
 */
@Component({
  selector: 'app-weight-field',
  imports: [MatFormFieldModule, MatInputModule, ReactiveFormsModule, TranslocoPipe],
  template: `
    <mat-form-field appearance="outline" subscriptSizing="dynamic" class="field">
      <mat-label>{{ 'hierarchy.links.weight' | transloco }}</mat-label>
      <input
        matInput
        inputmode="numeric"
        [formControl]="control"
        [attr.aria-label]="'hierarchy.links.weightOf' | transloco: { name: parentName() }"
        (change)="commit()"
      />
      <span matTextSuffix>%</span>
      <mat-error>{{ 'errors.field.out_of_range' | transloco }}</mat-error>
    </mat-form-field>
  `,
  styleUrl: './weight-field.scss',
})
export class WeightField {
  /** The saved weighting. */
  readonly weight = input.required<number>();
  /** Name of the parent, for the field's accessible name. */
  readonly parentName = input.required<string>();
  /** Read-only (the project is archived). */
  readonly disabled = input(false);
  /** A new valid weighting to save. */
  readonly weightChange = output<number>();

  protected readonly control = new FormControl('', { nonNullable: true, validators: weightValidator });

  constructor() {
    effect(() => {
      const weight = this.weight();
      untracked(() => this.control.reset(String(weight)));
    });
    effect(() => {
      if (this.disabled()) this.control.disable();
      else this.control.enable();
    });
  }

  protected commit(): void {
    this.control.markAsTouched();
    const weight = parseWeight(this.control.value);
    if (weight === null || weight === this.weight()) return;
    this.weightChange.emit(weight);
  }
}
