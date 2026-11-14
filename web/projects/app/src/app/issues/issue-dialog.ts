import { Component, computed, effect, inject, resource, signal } from '@angular/core';
import { toSignal } from '@angular/core/rxjs-interop';
import { AbstractControl, FormControl, FormGroup, FormRecord, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { DateAdapter, provideNativeDateAdapter } from '@angular/material/core';
import { MatDatepickerModule } from '@angular/material/datepicker';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatSelectModule } from '@angular/material/select';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ErrorState,
  ISSUE_PRIORITY,
  Issue,
  IssueFormField,
  IssuePerson,
  IssuePriority,
  LocaleService,
  Node,
  NodeItem,
  createIssue,
  getIssue,
  getIssueForm,
  isApiProblem,
  listNodes,
  toLocalDate,
} from 'ui';
import { RichTextDoc, RichTextEditor } from 'ui/rich-text';

import { placeApiErrors } from '../configuration/form-errors';
import { FieldInput, errorKeyOf } from './field-input';
import { FieldRaw, apiValue, controlOf, typesFor } from './issue-fields';
import { IssueDialogData } from './issue-flow';
import { PeoplePicker } from './people-picker';

/** Longest summary of a ticket. */
export const SUMMARY_MAX = 255;


/** An active project the member sees in full (not veiled), with its key. */
function isProject(item: NodeItem): item is Node & { key: string } {
  const node = item as Partial<Node>;
  return node.type === 'project' && !!node.key && node.archived === false;
}

/**
 * Creates a ticket (L05-US01): its project, its type, then the fields of
 * the type's creation screen as the API resolves them, required ones
 * marked. Closes with the ticket created.
 */
@Component({
  selector: 'app-issue-dialog',
  imports: [
    ErrorState,
    FieldInput,
    MatButtonModule,
    MatDatepickerModule,
    MatDialogModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    MatProgressBarModule,
    MatSelectModule,
    PeoplePicker,
    ReactiveFormsModule,
    RichTextEditor,
    TranslocoPipe,
  ],
  // Dates are picked in the user's locale; they travel as local dates.
  providers: [provideNativeDateAdapter()],
  templateUrl: './issue-dialog.html',
  styleUrl: './issue-dialog.scss',
})
export class IssueDialog {
  protected readonly data = inject<IssueDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<IssueDialog, Issue>>(MatDialogRef);
  private readonly idempotencyKey = crypto.randomUUID();

  protected readonly priorities = ISSUE_PRIORITY;
  protected readonly summaryMax = SUMMARY_MAX;
  /** The project is chosen in the dialog (from "My tickets"). */
  protected readonly choosesProject = !this.data.projectKey && !this.data.parent;
  protected readonly form = new FormGroup({
    project: new FormControl<string | null>(this.data.parent?.projectKey ?? this.data.projectKey ?? null, Validators.required),
    typeId: new FormControl<string | null>(null, Validators.required),
    summary: new FormControl('', { nonNullable: true, validators: [Validators.required, Validators.maxLength(SUMMARY_MAX)] }),
    description: new FormControl<RichTextDoc | null>(null),
    priority: new FormControl<IssuePriority>('medium', { nonNullable: true }),
    assignee: new FormControl<readonly IssuePerson[]>([], { nonNullable: true }),
    startDate: new FormControl<Date | null>(null),
    dueDate: new FormControl<Date | null>(null),
  });
  /** The controls of the custom fields, by field id. */
  protected readonly fieldControls = new FormRecord<FormControl<FieldRaw>>({});

  protected readonly projectKey = toSignal(this.form.controls.project.valueChanges, { initialValue: this.form.controls.project.value });
  private readonly typeId = toSignal(this.form.controls.typeId.valueChanges, { initialValue: null });

  protected readonly projects = resource({
    params: () => (this.choosesProject ? true : undefined),
    loader: () => this.api.invoke(listNodes, { type: 'project', limit: 200 }),
  });
  protected readonly projectChoices = computed(() => (this.projects.hasValue() ? this.projects.value().items.filter(isProject) : []));
  protected readonly issueForm = resource({
    params: () => {
      const key = this.projectKey();
      return key ? { key, typeId: this.typeId() ?? undefined } : undefined;
    },
    loader: ({ params }) => this.api.invoke(getIssueForm, params),
  });
  /** The types offered: under the parent, those it may hold; otherwise no sub-task. */
  protected readonly types = computed(() =>
    this.issueForm.hasValue() ? typesFor(this.issueForm.value().types, this.data.parent?.typeId ?? null) : [],
  );
  protected readonly fields = signal<readonly IssueFormField[]>([]);
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  constructor() {
    inject(DateAdapter).setLocale(inject(LocaleService).formattingLocale());
    // Another project: its own types and fields.
    this.form.controls.project.valueChanges.subscribe(() => {
      this.form.controls.typeId.setValue(null);
      for (const id of Object.keys(this.fieldControls.controls)) this.fieldControls.removeControl(id);
      this.fields.set([]);
    });
    // Each form read: the type it is for, and the controls of its fields
    // (a value kept when the next type has the same field).
    effect(() => {
      if (!this.issueForm.hasValue()) return;
      const form = this.issueForm.value();
      const offered = this.types();
      const chosen = offered.some((t) => t.id === this.form.controls.typeId.value) ? this.form.controls.typeId.value : null;
      if (!chosen) {
        // The type the API chose, if offered, else the first offered.
        const fallback = offered.find((t) => t.id === form.typeId) ?? offered[0];
        if (fallback && fallback.id !== form.typeId) {
          this.form.controls.typeId.setValue(fallback.id);
          return;
        }
        this.form.controls.typeId.setValue(fallback?.id ?? null, { emitEvent: false });
      }
      for (const id of Object.keys(this.fieldControls.controls)) {
        if (!form.fields.some((f) => f.fieldId === id)) this.fieldControls.removeControl(id);
      }
      for (const field of form.fields) {
        if (!this.fieldControls.contains(field.fieldId)) this.fieldControls.addControl(field.fieldId, controlOf(field));
      }
      this.fields.set(form.fields);
    });
  }

  protected controlOf(field: IssueFormField): FormControl<FieldRaw> {
    return this.fieldControls.controls[field.fieldId];
  }

  protected error(control: AbstractControl): string | null {
    return errorKeyOf(control as FormControl<unknown>);
  }

  protected forbidden(): boolean {
    const error = this.issueForm.error();
    return isApiProblem(error) && error.status === 403;
  }

  protected async save(): Promise<void> {
    const { startDate, dueDate } = this.form.getRawValue();
    this.form.controls.dueDate.setErrors(startDate && dueDate && dueDate < startDate ? { before_start: true } : null);
    if (this.form.invalid || this.fieldControls.invalid) {
      this.form.markAllAsTouched();
      this.fieldControls.markAllAsTouched();
      return;
    }
    const projectKey = this.projectKey();
    if (!projectKey) return;
    this.saving.set(true);
    this.errorKey.set(null);
    try {
      const fields = await this.fieldValues();
      if (!fields) return;
      const value = this.form.getRawValue();
      const issue = await this.api.invoke(createIssue, {
        key: projectKey,
        'Idempotency-Key': this.idempotencyKey,
        body: {
          typeId: value.typeId!,
          summary: value.summary.trim(),
          description: value.description,
          priority: value.priority,
          parentId: this.data.parent?.id ?? null,
          assigneeId: value.assignee[0]?.id ?? null,
          startDate: value.startDate ? toLocalDate(value.startDate) : null,
          dueDate: value.dueDate ? toLocalDate(value.dueDate) : null,
          fields,
        },
      });
      this.dialogRef.close(issue);
    } catch (error) {
      const controls: Record<string, AbstractControl> = {
        ...this.form.controls,
        assigneeId: this.form.controls.assignee,
      };
      for (const [id, control] of Object.entries(this.fieldControls.controls)) controls[`fields.${id}`] = control;
      this.errorKey.set(placeApiErrors(error, controls));
    } finally {
      this.saving.set(false);
    }
  }

  /** The custom field values to send, ticket keys resolved to ids; null if a key is unknown. */
  private async fieldValues(): Promise<Record<string, unknown> | null> {
    const values: Record<string, unknown> = {};
    let unknown = false;
    for (const field of this.fields()) {
      const control = this.controlOf(field);
      let value = apiValue(field, control.value);
      if (value === undefined) continue;
      if (field.kind === 'issue_link') {
        try {
          value = (await this.api.invoke(getIssue, { idOrKey: value as string })).id;
        } catch {
          control.setErrors({ api: 'errors.field.unknown_issue' });
          control.markAsTouched();
          unknown = true;
          continue;
        }
      }
      values[field.fieldId] = value;
    }
    return unknown ? null : values;
  }
}
