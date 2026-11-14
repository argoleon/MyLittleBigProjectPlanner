import { Component, computed, effect, inject, input, output, signal, untracked } from '@angular/core';
import { FormControl, FormGroup, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { MatSelectModule } from '@angular/material/select';
import { MatSlideToggleModule } from '@angular/material/slide-toggle';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ConfirmDialog,
  ConfirmDialogData,
  IssueType,
  IssueTypeColor,
  Workflow,
  TranslatableInput,
  TranslatedPipe,
  createIssueType,
  deleteIssueType,
  errorTranslationKey,
  fieldErrorTranslationKey,
  isApiProblem,
  translationLanguage,
  translationsForm,
  translationsValue,
  updateIssueType,
} from 'ui';

import { TYPE_COLORS, TYPE_ICONS, parentCandidates, typeFields } from './issue-types';

/** A key: a lowercase letter, then lowercase letters, digits or underscores. */
const KEY_PATTERN = /^[a-z][a-z0-9_]{0,39}$/;

/**
 * The panel of an issue type (mockup 12): its labels, identifier (set once),
 * icon, color, whether it is a sub-task, and the types it may sit under.
 * Saves to the draft of the configuration; removes the type after a
 * confirmation.
 */
@Component({
  selector: 'app-type-editor',
  imports: [
    MatButtonModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    MatSelectModule,
    MatSlideToggleModule,
    ReactiveFormsModule,
    TranslatableInput,
    TranslatedPipe,
    TranslocoPipe,
  ],
  styleUrl: './type-editor.scss',
  templateUrl: './type-editor.html',
})
export class TypeEditor {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);

  readonly configId = input.required<string>();
  /** The type changed, or null for a new one. */
  readonly type = input.required<IssueType | null>();
  /** Every type of the configuration. */
  readonly types = input.required<readonly IssueType[]>();
  /** The workflows a type may follow. */
  readonly workflows = input.required<readonly Workflow[]>();
  readonly saved = output<IssueType>();
  readonly removed = output<IssueType>();
  readonly closed = output<void>();

  protected readonly icons = TYPE_ICONS;
  protected readonly colors = TYPE_COLORS;
  protected readonly candidates = computed(() => parentCandidates(this.types(), this.type()?.id ?? null));
  protected readonly form = new FormGroup({
    label: translationsForm({}, { maxLength: 60 }),
    key: new FormControl('', { nonNullable: true, validators: [Validators.required, Validators.pattern(KEY_PATTERN)] }),
    icon: new FormControl('task_alt', { nonNullable: true }),
    color: new FormControl<IssueTypeColor>('task', { nonNullable: true }),
    isSubtask: new FormControl(false, { nonNullable: true }),
    parentIds: new FormControl<string[]>([], { nonNullable: true }),
    workflowId: new FormControl<string | null>(null),
  });
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  private idempotencyKey = crypto.randomUUID();

  constructor() {
    // A sub-task always has a parent type.
    this.form.controls.isSubtask.valueChanges.subscribe((subtask) => {
      this.form.controls.parentIds.setValidators(subtask ? Validators.required : null);
      this.form.controls.parentIds.updateValueAndValidity();
    });
    // Another type to edit: the form starts again from it.
    effect(() => {
      const type = this.type();
      untracked(() => this.reset(type));
    });
  }

  private reset(type: IssueType | null): void {
    this.errorKey.set(null);
    this.idempotencyKey = crypto.randomUUID();
    const label = translationsForm(type?.labelTranslations ?? {}, { maxLength: 60 });
    this.form.setControl('label', label);
    this.form.reset({
      key: type?.key ?? '',
      icon: type?.icon ?? 'task_alt',
      color: type?.color ?? 'task',
      isSubtask: type?.isSubtask ?? false,
      parentIds: [...(type?.parentIds ?? [])],
      // A new type follows the first workflow unless told otherwise.
      workflowId: type?.workflowId ?? this.workflows()[0]?.id ?? null,
    });
    if (type) this.form.controls.key.disable();
    else this.form.controls.key.enable();
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
    const fields = typeFields({ ...value, labelTranslations: translationsValue(this.form.controls.label) });
    const type = this.type();
    try {
      const saved = type
        ? await this.api.invoke(updateIssueType, {
            id: this.configId(),
            typeId: type.id,
            'If-Match': `"${type.version}"`,
            body: fields,
          })
        : await this.api.invoke(createIssueType, {
            id: this.configId(),
            'Idempotency-Key': this.idempotencyKey,
            body: { key: value.key, ...fields },
          });
      this.saved.emit(saved);
    } catch (error) {
      this.showError(error);
    } finally {
      this.saving.set(false);
    }
  }

  protected remove(type: IssueType): void {
    const data: ConfirmDialogData = {
      titleKey: 'config.types.remove.title',
      messageKey: 'config.types.remove.message',
      confirmKey: 'config.types.remove.confirm',
      params: { name: type.label ?? type.key },
    };
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, { data, width: 'min(480px, 100vw)' })
      .afterClosed()
      .subscribe(async (confirmed) => {
        if (!confirmed) return;
        this.errorKey.set(null);
        try {
          await this.api.invoke(deleteIssueType, { id: this.configId(), typeId: type.id, 'If-Match': `"${type.version}"` });
          this.removed.emit(type);
        } catch (error) {
          this.showError(error);
        }
      });
  }

  /** Field errors on their fields, the others above the form. */
  private showError(error: unknown): void {
    if (!isApiProblem(error)) {
      this.errorKey.set(errorTranslationKey('unknown'));
      return;
    }
    const labels = this.form.controls.label.controls;
    const placed = error.fieldErrors.filter((fieldError) => {
      const key = fieldErrorTranslationKey(fieldError.code);
      const language = translationLanguage(fieldError.field, 'labelTranslations');
      const controls =
        language !== null
          ? [labels[language]]
          : fieldError.field === 'labelTranslations'
            ? Object.values(labels)
            : fieldError.field === 'key' || fieldError.field === 'icon' || fieldError.field === 'parentIds'
              ? [this.form.controls[fieldError.field]]
              : [];
      for (const control of controls) {
        control.setErrors({ api: key });
        control.markAsTouched();
      }
      return controls.length > 0;
    });
    if (placed.length < error.fieldErrors.length || error.fieldErrors.length === 0) {
      this.errorKey.set(error.translationKey);
    }
  }
}
