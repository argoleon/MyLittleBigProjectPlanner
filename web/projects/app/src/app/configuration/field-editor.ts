import { CdkDrag, CdkDragDrop, CdkDragHandle, CdkDropList } from '@angular/cdk/drag-drop';
import { Component, computed, effect, inject, input, output, signal, untracked } from '@angular/core';
import { FormArray, FormControl, FormGroup, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { MatMenuModule } from '@angular/material/menu';
import { MatSelectModule } from '@angular/material/select';
import { MatSlideToggleModule } from '@angular/material/slide-toggle';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ConfigField,
  ConfirmDialog,
  ConfirmDialogData,
  FieldKind,
  FieldOptionsInput,
  FieldRule,
  FieldUnit,
  IssueType,
  LANGUAGES,
  LANGUAGE_NAMES,
  TranslatableInput,
  TranslatedPipe,
  Workflow,
  createField,
  deleteField,
  translationsForm,
  translationsValue,
  updateField,
} from 'ui';

import {
  Applicability,
  FIELD_KINDS,
  FIELD_LABEL_MAX,
  MAX_DECIMALS,
  SHORT_TEXT_MAX,
  UNITS,
  hasValues,
  kindIcon,
  rulesFor,
} from './fields';
import { ValueForm, moveValue, removeValue, valueForm, valuesForm, valuesInput } from './field-values';
import { placeApiErrors } from './form-errors';
import { RulesGrid } from './rules-grid';

/** A key: a lowercase letter, then lowercase letters, digits or underscores. */
const KEY_PATTERN = /^[a-z][a-z0-9_]{0,39}$/;

/**
 * The panel of a field (mockup 12): its labels, identifier and type (set
 * once), the options of its type, its list values (reordered, deactivated
 * once published), the types it applies to, and its rules grid. A system
 * field keeps its labels and options: only its types and rules change.
 * Saves to the draft of the configuration.
 */
@Component({
  selector: 'app-field-editor',
  imports: [
    CdkDrag,
    CdkDragHandle,
    CdkDropList,
    MatButtonModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    MatMenuModule,
    MatSelectModule,
    MatSlideToggleModule,
    ReactiveFormsModule,
    RulesGrid,
    TranslatableInput,
    TranslatedPipe,
    TranslocoPipe,
  ],
  styleUrl: './field-editor.scss',
  templateUrl: './field-editor.html',
})
export class FieldEditor {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);

  readonly configId = input.required<string>();
  /** The field changed, or null for a new one. */
  readonly field = input.required<ConfigField | null>();
  /** Every type of the configuration, in their order. */
  readonly types = input.required<readonly IssueType[]>();
  readonly workflows = input.required<readonly Workflow[]>();
  readonly saved = output<ConfigField>();
  readonly removed = output<ConfigField>();
  readonly closed = output<void>();

  protected readonly kinds = FIELD_KINDS;
  protected readonly units = UNITS;
  protected readonly decimals = Array.from({ length: MAX_DECIMALS + 1 }, (_, i) => i);
  protected readonly languages = LANGUAGES;
  protected readonly languageNames = LANGUAGE_NAMES;
  protected readonly labelMax = FIELD_LABEL_MAX;
  protected readonly shortTextMax = SHORT_TEXT_MAX;
  protected readonly kindIcon = kindIcon;
  protected readonly form = new FormGroup({
    label: translationsForm({}, { maxLength: FIELD_LABEL_MAX }),
    key: new FormControl('', { nonNullable: true, validators: [Validators.required, Validators.pattern(KEY_PATTERN)] }),
    kind: new FormControl<FieldKind>('short_text', { nonNullable: true }),
    maxLength: new FormControl<number | null>(null, [Validators.min(1), Validators.max(SHORT_TEXT_MAX)]),
    decimals: new FormControl(0, { nonNullable: true }),
    min: new FormControl<number | null>(null),
    max: new FormControl<number | null>(null),
    unit: new FormControl<FieldUnit | null>(null),
    multiple: new FormControl(false, { nonNullable: true }),
    values: new FormArray<ValueForm>([]),
    allTypes: new FormControl(false, { nonNullable: true }),
    issueTypeIds: new FormControl<string[]>([], { nonNullable: true }),
  });
  /** The cells of the rules grid that are not Visible. */
  protected readonly rules = signal<FieldRule[]>([]);
  protected readonly kind = signal<FieldKind>('short_text');
  protected readonly applicability = signal<Applicability>({ allTypes: false, issueTypeIds: [] });
  protected readonly isSystem = computed(() => this.field()?.isSystem ?? false);
  protected readonly withValues = computed(() => hasValues(this.kind()));
  protected readonly cascading = computed(() => this.kind() === 'cascading_select');
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  private idempotencyKey = crypto.randomUUID();

  constructor() {
    this.form.controls.kind.valueChanges.subscribe((kind) => {
      this.kind.set(kind);
      // A list starts with one value to fill in.
      if (hasValues(kind) && this.form.controls.values.length === 0) this.addValue(this.form.controls.values);
    });
    const applicability = () =>
      this.applicability.set({
        allTypes: this.form.controls.allTypes.value,
        issueTypeIds: this.form.controls.issueTypeIds.value,
      });
    this.form.controls.allTypes.valueChanges.subscribe(applicability);
    this.form.controls.issueTypeIds.valueChanges.subscribe(applicability);
    // Another field to edit: the form starts again from it.
    effect(() => {
      const field = this.field();
      untracked(() => this.reset(field));
    });
  }

  private reset(field: ConfigField | null): void {
    this.errorKey.set(null);
    this.idempotencyKey = crypto.randomUUID();
    this.form.setControl('label', translationsForm(field?.labelTranslations ?? {}, { maxLength: FIELD_LABEL_MAX }));
    const options = field?.options;
    this.form.reset(
      {
        key: field?.key ?? '',
        kind: field?.kind ?? 'short_text',
        maxLength: options?.maxLength ?? null,
        decimals: options?.decimals ?? 0,
        min: options?.min ?? null,
        max: options?.max ?? null,
        unit: options?.unit ?? null,
        multiple: options?.multiple ?? false,
        allTypes: field?.allTypes ?? false,
        issueTypeIds: [...(field?.issueTypeIds ?? [])],
      },
      { emitEvent: false },
    );
    this.form.setControl('values', valuesForm(options?.values), { emitEvent: false });
    this.kind.set(this.form.controls.kind.value);
    this.applicability.set({ allTypes: this.form.controls.allTypes.value, issueTypeIds: this.form.controls.issueTypeIds.value });
    this.rules.set([...(field?.rules ?? [])]);
    // Set once: the identifier and the type; a system field keeps its labels and options too.
    for (const name of ['key', 'kind'] as const) {
      if (field) this.form.controls[name].disable({ emitEvent: false });
      else this.form.controls[name].enable({ emitEvent: false });
    }
    const fixed = field?.isSystem ?? false;
    for (const control of [
      this.form.controls.label,
      this.form.controls.maxLength,
      this.form.controls.decimals,
      this.form.controls.min,
      this.form.controls.max,
      this.form.controls.unit,
      this.form.controls.multiple,
      this.form.controls.values,
    ]) {
      if (fixed) control.disable({ emitEvent: false });
      else control.enable({ emitEvent: false });
    }
  }

  protected addValue(values: FormArray<ValueForm>): void {
    values.push(valueForm());
    values.markAsDirty();
  }

  protected removeValue(values: FormArray<ValueForm>, index: number): void {
    removeValue(values, index);
  }

  protected reactivate(value: ValueForm): void {
    value.controls.active.setValue(true);
    value.markAsDirty();
  }

  protected moveValue(values: FormArray<ValueForm>, from: number, to: number): void {
    moveValue(values, from, to);
  }

  protected drop(values: FormArray<ValueForm>, event: CdkDragDrop<unknown>): void {
    moveValue(values, event.previousIndex, event.currentIndex);
  }

  /** The options of the field's type, as the API takes them. */
  private options(): FieldOptionsInput {
    const field = this.field();
    if (field?.isSystem) return this.systemOptions(field);
    const value = this.form.getRawValue();
    switch (value.kind) {
      case 'short_text':
        return value.maxLength !== null ? { maxLength: value.maxLength } : {};
      case 'number':
        return {
          decimals: value.decimals,
          ...(value.min !== null ? { min: value.min } : {}),
          ...(value.max !== null ? { max: value.max } : {}),
          ...(value.unit ? { unit: value.unit } : {}),
        };
      case 'user':
        return { multiple: value.multiple };
      case 'single_select':
      case 'multi_select':
      case 'cascading_select':
        return { values: valuesInput(this.form.controls.values, value.kind === 'cascading_select') };
      default:
        return {};
    }
  }

  /** A system field sends back its options as they are. */
  private systemOptions(field: ConfigField): FieldOptionsInput {
    return { ...field.options, values: undefined };
  }

  protected async save(): Promise<void> {
    for (const control of Object.values(this.form.controls.label.controls)) control.updateValueAndValidity();
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    const value = this.form.getRawValue();
    const field = this.field();
    const applicability = { allTypes: value.allTypes, issueTypeIds: value.allTypes ? [] : value.issueTypeIds };
    const settings = {
      labelTranslations: field?.isSystem ? field.labelTranslations : translationsValue(this.form.controls.label),
      options: this.options(),
      ...applicability,
      rules: rulesFor(this.rules(), applicability),
    };
    try {
      const saved = field
        ? await this.api.invoke(updateField, {
            id: this.configId(),
            fieldId: field.id,
            'If-Match': `"${field.version}"`,
            body: settings,
          })
        : await this.api.invoke(createField, {
            id: this.configId(),
            'Idempotency-Key': this.idempotencyKey,
            body: { key: value.key, kind: value.kind, ...settings },
          });
      this.saved.emit(saved);
    } catch (error) {
      this.errorKey.set(placeApiErrors(error, this.form.controls));
    } finally {
      this.saving.set(false);
    }
  }

  protected remove(field: ConfigField): void {
    const data: ConfirmDialogData = {
      titleKey: 'config.fields.remove.title',
      messageKey: 'config.fields.remove.message',
      confirmKey: 'config.fields.remove.confirm',
      params: { name: field.label ?? field.key },
    };
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, { data, width: 'min(480px, 100vw)' })
      .afterClosed()
      .subscribe(async (confirmed) => {
        if (!confirmed) return;
        this.errorKey.set(null);
        try {
          await this.api.invoke(deleteField, { id: this.configId(), fieldId: field.id, 'If-Match': `"${field.version}"` });
          this.removed.emit(field);
        } catch (error) {
          this.errorKey.set(placeApiErrors(error, {}));
        }
      });
  }
}
