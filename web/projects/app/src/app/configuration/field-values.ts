import { moveItemInArray } from '@angular/cdk/drag-drop';
import { FormArray, FormControl, FormGroup } from '@angular/forms';
import { FieldValue, FieldValueInput, TranslationsForm, translationsForm, translationsValue } from 'ui';

import { FIELD_LABEL_MAX } from './fields';

/** The controls of a list value: its identity, whether published and active, its labels, the values under it. */
export interface ValueControls {
  id: FormControl<string | null>;
  published: FormControl<boolean>;
  active: FormControl<boolean>;
  label: TranslationsForm;
  children: FormArray<FormGroup<ValueControls>>;
}

export type ValueForm = FormGroup<ValueControls>;

/** The form of a value: as the API returned it, or a new one. */
export function valueForm(value?: FieldValue): ValueForm {
  return new FormGroup<ValueControls>({
    // Kept by a reset of the form: its initial value is its identity.
    id: new FormControl<string | null>(value?.id ?? null, { nonNullable: true }),
    published: new FormControl(value?.published ?? false, { nonNullable: true }),
    active: new FormControl(value?.active ?? true, { nonNullable: true }),
    label: translationsForm(value?.labelTranslations ?? {}, { maxLength: FIELD_LABEL_MAX }),
    children: new FormArray((value?.children ?? []).map((child) => valueForm(child))),
  });
}

/** The forms of `values`. */
export function valuesForm(values: readonly FieldValue[] | null | undefined): FormArray<ValueForm> {
  return new FormArray((values ?? []).map((value) => valueForm(value)));
}

/**
 * Removes the value at `index`: a value of the published version is
 * deactivated (tickets may hold it), one added since goes for good.
 */
export function removeValue(values: FormArray<ValueForm>, index: number): void {
  const value = values.at(index);
  if (value.controls.published.value) value.controls.active.setValue(false);
  else values.removeAt(index);
  values.markAsDirty();
}

/** Moves the value at `from` to `to`. */
export function moveValue(values: FormArray<ValueForm>, from: number, to: number): void {
  if (from === to || to < 0 || to >= values.length) return;
  const controls = [...values.controls];
  moveItemInArray(controls, from, to);
  values.clear({ emitEvent: false });
  for (const control of controls) values.push(control, { emitEvent: false });
  values.updateValueAndValidity();
  values.markAsDirty();
}

/** What the API expects; the values under a value only for a cascading select. */
export function valuesInput(values: FormArray<ValueForm>, cascading: boolean): FieldValueInput[] {
  return values.controls.map((value) => ({
    ...(value.controls.id.value ? { id: value.controls.id.value } : {}),
    labelTranslations: translationsValue(value.controls.label),
    active: value.controls.active.value,
    children: cascading ? valuesInput(value.controls.children, false) : [],
  }));
}
