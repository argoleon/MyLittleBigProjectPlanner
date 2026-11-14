import { Component, computed, inject, resource, signal } from '@angular/core';
import { FormControl, FormGroup, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { MatSelectModule } from '@angular/material/select';
import { MatSlideToggleModule } from '@angular/material/slide-toggle';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ConfigField,
  TransitionRuleReferences,
  TranslatableInput,
  TranslatedPipe,
  Workflow,
  WorkflowCategory,
  WorkflowStatus,
  WorkflowTransition,
  createWorkflow,
  createWorkflowStatus,
  createWorkflowTransition,
  listTransitionRoles,
  translationsForm,
  translationsValue,
  updateWorkflow,
  updateWorkflowStatus,
  updateWorkflowTransition,
} from 'ui';

import { placeApiErrors } from './form-errors';
import { TransitionRulesEditor } from './transition-rules-editor';
import { placeRuleErrors, rulesForm, rulesValue } from './transition-rules';

/** A key: a lowercase letter, then lowercase letters, digits or underscores. */
export const KEY_PATTERN = /^[a-z][a-z0-9_]{0,39}$/;

/** The value of "All statuses" in the form of a transition; null for the API. */
export const ANY_STATUS = '*';

/** The categories of a status, in their order. */
export const CATEGORIES: readonly WorkflowCategory[] = ['todo', 'in_progress', 'done'];

/** A new workflow (`workflow` absent), or one renamed. */
export interface WorkflowDialogData {
  readonly configId: string;
  readonly workflow?: Workflow;
  /** The workflows a new one may copy. */
  readonly workflows: readonly Workflow[];
}

/** Adds a workflow, empty or a copy, or renames one. Closes with the workflow saved. */
@Component({
  selector: 'app-workflow-dialog',
  imports: [MatButtonModule, MatDialogModule, MatFormFieldModule, MatInputModule, MatSelectModule, ReactiveFormsModule, TranslatableInput, TranslatedPipe, TranslocoPipe],
  styleUrl: './config-dialog.scss',
  template: `
    <h2 mat-dialog-title>{{ (data.workflow ? 'config.workflows.renameTitle' : 'config.workflows.newTitle') | transloco }}</h2>
    <form [formGroup]="form" (ngSubmit)="save()" novalidate>
      <mat-dialog-content class="fields">
        @if (errorKey(); as key) {
          <p class="message error" role="alert">{{ key | transloco }}</p>
        }
        <ui-translatable-input [group]="form.controls.labelTranslations" [label]="'config.workflows.label' | transloco" [maxLength]="60" />
        @if (!data.workflow) {
          <mat-form-field appearance="outline">
            <mat-label>{{ 'config.types.key' | transloco }}</mat-label>
            <input matInput formControlName="key" maxlength="40" required autocomplete="off" />
            <mat-hint>{{ 'config.types.keyHint' | transloco }}</mat-hint>
            <mat-error>{{ (form.controls.key.getError('api') ?? 'config.types.keyFormat') | transloco }}</mat-error>
          </mat-form-field>
          <mat-form-field appearance="outline">
            <mat-label>{{ 'config.workflows.copyOf' | transloco }}</mat-label>
            <mat-select formControlName="copyOf">
              <mat-option [value]="null">{{ 'config.workflows.empty' | transloco }}</mat-option>
              @for (workflow of data.workflows; track workflow.id) {
                <mat-option [value]="workflow.id">{{ workflow.labelTranslations | translated }}</mat-option>
              }
            </mat-select>
          </mat-form-field>
        }
      </mat-dialog-content>
      <mat-dialog-actions align="end">
        <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
        <button matButton="filled" type="submit" class="confirm" [disabled]="saving()">{{ 'config.types.save' | transloco }}</button>
      </mat-dialog-actions>
    </form>
  `,
})
export class WorkflowDialog {
  protected readonly data = inject<WorkflowDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<WorkflowDialog, Workflow>>(MatDialogRef);
  private readonly idempotencyKey = crypto.randomUUID();

  protected readonly form = new FormGroup({
    labelTranslations: translationsForm(this.data.workflow?.labelTranslations ?? {}, { maxLength: 60 }),
    key: new FormControl(this.data.workflow?.key ?? '', { nonNullable: true, validators: [Validators.required, Validators.pattern(KEY_PATTERN)] }),
    copyOf: new FormControl<string | null>(null),
  });
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  constructor() {
    if (this.data.workflow) this.form.controls.key.disable();
  }

  protected async save(): Promise<void> {
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    const labelTranslations = translationsValue(this.form.controls.labelTranslations);
    const { key, copyOf } = this.form.getRawValue();
    try {
      const saved = this.data.workflow
        ? await this.api.invoke(updateWorkflow, {
            id: this.data.configId,
            workflowId: this.data.workflow.id,
            'If-Match': `"${this.data.workflow.version}"`,
            body: { labelTranslations },
          })
        : await this.api.invoke(createWorkflow, {
            id: this.data.configId,
            'Idempotency-Key': this.idempotencyKey,
            body: { key, labelTranslations, copyOf },
          });
      this.dialogRef.close(saved);
    } catch (error) {
      this.errorKey.set(placeApiErrors(error, this.form.controls));
    } finally {
      this.saving.set(false);
    }
  }
}

/** A new status of `workflow` (`status` absent), or one changed. */
export interface StatusDialogData {
  readonly configId: string;
  readonly workflow: Workflow;
  readonly status?: WorkflowStatus;
}

/** Adds or changes a status: labels, portal label, category, initial. Closes with its workflow. */
@Component({
  selector: 'app-status-dialog',
  imports: [MatButtonModule, MatDialogModule, MatFormFieldModule, MatInputModule, MatSelectModule, MatSlideToggleModule, ReactiveFormsModule, TranslatableInput, TranslocoPipe],
  styleUrl: './config-dialog.scss',
  template: `
    <h2 mat-dialog-title>{{ (data.status ? 'config.workflows.status.editTitle' : 'config.workflows.status.newTitle') | transloco }}</h2>
    <form [formGroup]="form" (ngSubmit)="save()" novalidate>
      <mat-dialog-content class="fields">
        @if (errorKey(); as key) {
          <p class="message error" role="alert">{{ key | transloco }}</p>
        }
        <ui-translatable-input [group]="form.controls.labelTranslations" [label]="'config.types.label' | transloco" [maxLength]="60" />
        <mat-form-field appearance="outline">
          <mat-label>{{ 'config.types.key' | transloco }}</mat-label>
          <input matInput formControlName="key" maxlength="40" required autocomplete="off" />
          <mat-hint>{{ (data.status ? 'config.types.keyFixed' : 'config.types.keyHint') | transloco }}</mat-hint>
          <mat-error>{{ (form.controls.key.getError('api') ?? 'config.types.keyFormat') | transloco }}</mat-error>
        </mat-form-field>
        <mat-form-field appearance="outline">
          <mat-label>{{ 'config.workflows.status.category' | transloco }}</mat-label>
          <mat-select formControlName="category">
            @for (category of categories; track category) {
              <mat-option [value]="category">{{ 'config.workflows.categories.' + category | transloco }}</mat-option>
            }
          </mat-select>
        </mat-form-field>
        <ui-translatable-input
          [group]="form.controls.portalLabelTranslations"
          [label]="'config.workflows.status.portalLabel' | transloco"
          [fallback]="null"
          [maxLength]="60"
        />
        <mat-slide-toggle formControlName="isInitial">{{ 'config.workflows.status.initial' | transloco }}</mat-slide-toggle>
      </mat-dialog-content>
      <mat-dialog-actions align="end">
        <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
        <button matButton="filled" type="submit" class="confirm" [disabled]="saving()">{{ 'config.types.save' | transloco }}</button>
      </mat-dialog-actions>
    </form>
  `,
})
export class StatusDialog {
  protected readonly data = inject<StatusDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<StatusDialog, Workflow>>(MatDialogRef);
  private readonly idempotencyKey = crypto.randomUUID();
  protected readonly categories = CATEGORIES;

  protected readonly form = new FormGroup({
    labelTranslations: translationsForm(this.data.status?.labelTranslations ?? {}, { maxLength: 60 }),
    portalLabelTranslations: translationsForm(this.data.status?.portalLabelTranslations ?? {}, { maxLength: 60, required: false }),
    key: new FormControl(this.data.status?.key ?? '', { nonNullable: true, validators: [Validators.required, Validators.pattern(KEY_PATTERN)] }),
    category: new FormControl<WorkflowCategory>(this.data.status?.category ?? 'todo', { nonNullable: true }),
    isInitial: new FormControl(this.data.status?.isInitial ?? false, { nonNullable: true }),
  });
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  constructor() {
    if (this.data.status) this.form.controls.key.disable();
  }

  protected async save(): Promise<void> {
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    const { key, category, isInitial } = this.form.getRawValue();
    const fields = {
      labelTranslations: translationsValue(this.form.controls.labelTranslations),
      portalLabelTranslations: translationsValue(this.form.controls.portalLabelTranslations),
      category,
      isInitial,
    };
    try {
      const status = this.data.status;
      const saved = status
        ? await this.api.invoke(updateWorkflowStatus, {
            id: this.data.configId,
            statusId: status.id,
            'If-Match': `"${status.version}"`,
            body: fields,
          })
        : await this.api.invoke(createWorkflowStatus, {
            id: this.data.configId,
            workflowId: this.data.workflow.id,
            'Idempotency-Key': this.idempotencyKey,
            body: { key, ...fields },
          });
      this.dialogRef.close(saved);
    } catch (error) {
      this.errorKey.set(placeApiErrors(error, this.form.controls));
    } finally {
      this.saving.set(false);
    }
  }
}

/** A new transition of `workflow` (`transition` absent), or one changed. */
export interface TransitionDialogData {
  readonly configId: string;
  readonly workflow: Workflow;
  readonly transition?: WorkflowTransition;
  /** The fields of the configuration, for the rules. */
  readonly fields: readonly ConfigField[];
  /** The names of the roles, groups and members the rules name. */
  readonly references: TransitionRuleReferences | null;
}

/**
 * Adds or changes a transition: its name, from a status or from all, to a
 * status, and its rules (L04-US05). Closes with its workflow.
 */
@Component({
  selector: 'app-transition-dialog',
  imports: [
    MatButtonModule,
    MatDialogModule,
    MatFormFieldModule,
    MatSelectModule,
    ReactiveFormsModule,
    TransitionRulesEditor,
    TranslatableInput,
    TranslatedPipe,
    TranslocoPipe,
  ],
  styleUrl: './config-dialog.scss',
  template: `
    <h2 mat-dialog-title>{{ (data.transition ? 'config.workflows.transition.editTitle' : 'config.workflows.transition.newTitle') | transloco }}</h2>
    <form [formGroup]="form" (ngSubmit)="save()" novalidate>
      <mat-dialog-content class="fields">
        @if (errorKey(); as key) {
          <p class="message error" role="alert">{{ key | transloco }}</p>
        }
        <ui-translatable-input [group]="form.controls.nameTranslations" [label]="'config.workflows.transition.name' | transloco" [maxLength]="60" />
        <mat-form-field appearance="outline">
          <mat-label>{{ 'config.workflows.transition.from' | transloco }}</mat-label>
          <mat-select formControlName="fromId">
            <mat-option [value]="any">{{ 'config.workflows.transition.any' | transloco }}</mat-option>
            @for (option of statuses(); track option.id) {
              <mat-option [value]="option.id">{{ option.labelTranslations | translated }}</mat-option>
            }
          </mat-select>
        </mat-form-field>
        <mat-form-field appearance="outline">
          <mat-label>{{ 'config.workflows.transition.to' | transloco }}</mat-label>
          <mat-select formControlName="toId" required>
            @for (option of statuses(); track option.id) {
              <mat-option [value]="option.id">{{ option.labelTranslations | translated }}</mat-option>
            }
          </mat-select>
          <mat-error>{{ (form.controls.toId.getError('api') ?? 'errors.field.required') | transloco }}</mat-error>
        </mat-form-field>
        <app-transition-rules [form]="form.controls.rules" [roles]="roles()" [fields]="data.fields" />
      </mat-dialog-content>
      <mat-dialog-actions align="end">
        <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
        <button matButton="filled" type="submit" class="confirm" [disabled]="saving()">{{ 'config.types.save' | transloco }}</button>
      </mat-dialog-actions>
    </form>
  `,
})
export class TransitionDialog {
  protected readonly data = inject<TransitionDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<TransitionDialog, Workflow>>(MatDialogRef);
  private readonly idempotencyKey = crypto.randomUUID();
  protected readonly statuses = computed(() => this.data.workflow.statuses);
  /** "All statuses" in the form: a select shows nothing for a null value. */
  protected readonly any = ANY_STATUS;

  protected readonly form = new FormGroup({
    nameTranslations: translationsForm(this.data.transition?.nameTranslations ?? {}, { maxLength: 60 }),
    fromId: new FormControl<string>(this.data.transition?.fromId ?? ANY_STATUS, { nonNullable: true }),
    toId: new FormControl<string | null>(this.data.transition?.toId ?? null, { validators: Validators.required }),
    rules: rulesForm(this.data.transition?.rules, this.data.references, this.data.fields),
  });
  private readonly roleList = resource({ loader: () => this.api.invoke(listTransitionRoles, {}) });
  /** The roles a transition can be restricted to. */
  protected readonly roles = computed(() => (this.roleList.hasValue() ? this.roleList.value() : []));
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  protected async save(): Promise<void> {
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    const { fromId, toId } = this.form.getRawValue();
    const body = {
      nameTranslations: translationsValue(this.form.controls.nameTranslations),
      fromId: fromId === ANY_STATUS ? null : fromId,
      toId: toId!,
      rules: rulesValue(this.form.controls.rules, this.data.fields),
    };
    try {
      const transition = this.data.transition;
      const saved = transition
        ? await this.api.invoke(updateWorkflowTransition, {
            id: this.data.configId,
            transitionId: transition.id,
            'If-Match': `"${transition.version}"`,
            body,
          })
        : await this.api.invoke(createWorkflowTransition, {
            id: this.data.configId,
            workflowId: this.data.workflow.id,
            'Idempotency-Key': this.idempotencyKey,
            body,
          });
      this.dialogRef.close(saved);
    } catch (error) {
      const left = placeApiErrors(error, this.form.controls);
      this.errorKey.set(placeRuleErrors(error, this.form.controls.rules) ? null : left);
    } finally {
      this.saving.set(false);
    }
  }
}
