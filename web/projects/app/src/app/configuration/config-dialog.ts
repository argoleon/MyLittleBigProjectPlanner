import { Component, inject, signal } from '@angular/core';
import { FormGroup, ReactiveFormsModule } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ConfigProject,
  ProjectConfig,
  TranslatableInput,
  Translations,
  createProjectConfig,
  duplicateProjectConfig,
  errorTranslationKey,
  fieldErrorTranslationKey,
  isApiProblem,
  translationLanguage,
  translationsForm,
  translationsValue,
} from 'ui';

/**
 * What the dialog does: create a configuration, duplicate `source`, or
 * detach `project` from `source` (its own copy).
 */
export type ConfigDialogData =
  | { readonly mode: 'create' }
  | { readonly mode: 'duplicate'; readonly source: ProjectConfig }
  | { readonly mode: 'detach'; readonly source: ProjectConfig; readonly project: ConfigProject };

/**
 * The name a copy starts with, in each language of the original: suffixed
 * with the key of the project it detaches, else with "(2)". Names are
 * unique, so the user may change it.
 */
export function copyName(data: ConfigDialogData): Translations {
  if (data.mode === 'create') return {};
  const suffix = data.mode === 'detach' ? data.project.key : '2';
  return Object.fromEntries(
    Object.entries(data.source.nameTranslations)
      .filter(([, name]) => name?.trim())
      .map(([language, name]) => [language, `${name?.trim()} (${suffix})`]),
  );
}

/** Creates, duplicates or detaches a project configuration (L04-US01). Closes with the configuration created. */
@Component({
  selector: 'app-config-dialog',
  imports: [MatButtonModule, MatDialogModule, ReactiveFormsModule, TranslatableInput, TranslocoPipe],
  styleUrl: './config-dialog.scss',
  template: `
    <h2 mat-dialog-title>{{ 'config.dialog.' + data.mode + '.title' | transloco: params }}</h2>
    <form [formGroup]="form" (ngSubmit)="save()" novalidate>
      <mat-dialog-content class="fields">
        @if (data.mode !== 'create') {
          <p class="intro">{{ 'config.dialog.' + data.mode + '.intro' | transloco: params }}</p>
        }
        @if (errorKey(); as key) {
          <p class="message error" role="alert">{{ key | transloco }}</p>
        }
        <ui-translatable-input [group]="form.controls.name" [label]="'config.name' | transloco" />
      </mat-dialog-content>
      <mat-dialog-actions align="end">
        <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
        <button matButton="filled" type="submit" class="confirm" [disabled]="saving()">
          {{ 'config.dialog.' + data.mode + '.confirm' | transloco }}
        </button>
      </mat-dialog-actions>
    </form>
  `,
})
export class ConfigDialog {
  protected readonly data = inject<ConfigDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<ConfigDialog, ProjectConfig>>(MatDialogRef);

  protected readonly form = new FormGroup({ name: translationsForm(copyName(this.data)) });
  protected readonly params: Record<string, string> =
    this.data.mode === 'create'
      ? {}
      : { name: this.data.source.name ?? '', project: this.data.mode === 'detach' ? this.data.project.name : '' };
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  private readonly idempotencyKey = crypto.randomUUID();

  protected async save(): Promise<void> {
    // An error of the API (a name taken) holds until the next try.
    for (const control of Object.values(this.form.controls.name.controls)) control.updateValueAndValidity();
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    const nameTranslations = translationsValue(this.form.controls.name);
    try {
      const saved =
        this.data.mode === 'create'
          ? await this.api.invoke(createProjectConfig, { 'Idempotency-Key': this.idempotencyKey, body: { nameTranslations } })
          : await this.api.invoke(duplicateProjectConfig, {
              id: this.data.source.id,
              'Idempotency-Key': this.idempotencyKey,
              body: { nameTranslations, projectId: this.data.mode === 'detach' ? this.data.project.id : null },
            });
      this.dialogRef.close(saved);
    } catch (error) {
      this.showError(error);
    } finally {
      this.saving.set(false);
    }
  }

  /** A name error goes on its field: in every language when taken, else in the one it names. */
  private showError(error: unknown): void {
    const nameErrors = isApiProblem(error) ? error.fieldErrors.filter((e) => e.field.startsWith('nameTranslations')) : [];
    if (nameErrors.length === 0) {
      this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
      return;
    }
    const controls = this.form.controls.name.controls;
    for (const fieldError of nameErrors) {
      const language = translationLanguage(fieldError.field, 'nameTranslations');
      for (const control of language ? [controls[language]] : Object.values(controls)) {
        control.setErrors({ api: fieldErrorTranslationKey(fieldError.code) });
        control.markAsTouched();
      }
    }
  }
}
