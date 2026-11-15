import { Component, inject, resource, signal } from '@angular/core';
import { AbstractControl, FormControl, FormGroup, FormRecord, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { DateAdapter, provideNativeDateAdapter } from '@angular/material/core';
import { MatDatepickerModule } from '@angular/material/datepicker';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatSelectModule } from '@angular/material/select';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ErrorState,
  ISSUE_PRIORITY,
  Issue,
  IssueChanges,
  IssueFormField,
  IssuePerson,
  IssuePriority,
  LocaleService,
  fromLocalDate,
  getIssue,
  getIssueEditForm,
  isApiProblem,
  toLocalDate,
  updateIssue,
} from 'ui';
import { RichTextDoc, RichTextEditor } from 'ui/rich-text';

import { placeApiErrors } from '../configuration/form-errors';
import { FieldInput, errorKeyOf } from './field-input';
import { SUMMARY_MAX } from './issue-dialog';
import { FieldRaw, apiValue, controlOf, rawOf } from './issue-fields';
import { PeoplePicker } from './people-picker';

/** How the edit dialog closes: the ticket saved, or a reload asked for after a conflict. */
export type EditOutcome = { readonly saved: Issue } | { readonly reload: true };

/**
 * Edits a ticket (L05-US02): its summary, description, priority,
 * assignee and dates, and the fields of its type's view screen at its
 * status. Only what changed is sent, with the version read: if someone
 * saved first, the dialog says so and offers to reload.
 */
@Component({
  selector: 'app-issue-edit-dialog',
  imports: [
    ErrorState,
    FieldInput,
    MatButtonModule,
    MatDatepickerModule,
    MatDialogModule,
    MatFormFieldModule,
    MatInputModule,
    MatProgressBarModule,
    MatSelectModule,
    PeoplePicker,
    ReactiveFormsModule,
    RichTextEditor,
    TranslocoPipe,
  ],
  providers: [provideNativeDateAdapter()],
  templateUrl: './issue-edit-dialog.html',
  styleUrl: './issue-dialog.scss',
})
export class IssueEditDialog {
  protected readonly ticket = inject<Issue>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<IssueEditDialog, EditOutcome>>(MatDialogRef);

  protected readonly priorities = ISSUE_PRIORITY;
  protected readonly summaryMax = SUMMARY_MAX;
  protected readonly form = new FormGroup({
    summary: new FormControl(this.ticket.summary, { nonNullable: true, validators: [Validators.required, Validators.maxLength(SUMMARY_MAX)] }),
    description: new FormControl<RichTextDoc | null>(this.ticket.description as RichTextDoc | null),
    priority: new FormControl<IssuePriority>(this.ticket.priority, { nonNullable: true }),
    assignee: new FormControl<readonly IssuePerson[]>(this.ticket.assignee ? [this.ticket.assignee] : [], { nonNullable: true }),
    startDate: new FormControl<Date | null>(fromLocalDate(this.ticket.startDate)),
    dueDate: new FormControl<Date | null>(fromLocalDate(this.ticket.dueDate)),
  });
  /** The controls of the custom fields, by field id. */
  protected readonly fieldControls = new FormRecord<FormControl<FieldRaw>>({});
  protected readonly editForm = resource({
    loader: async () => {
      const form = await this.api.invoke(getIssueEditForm, { idOrKey: this.ticket.id });
      for (const field of form.fields) {
        const control = controlOf(field);
        control.setValue(rawOf(field, this.ticket.fields.find((v) => v.fieldId === field.fieldId)));
        this.fieldControls.addControl(field.fieldId, control);
      }
      return form;
    },
  });
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  /** Someone saved the ticket since it was read. */
  protected readonly conflict = signal(false);

  constructor() {
    inject(DateAdapter).setLocale(inject(LocaleService).formattingLocale());
  }

  protected fields(): readonly IssueFormField[] {
    return this.editForm.hasValue() ? this.editForm.value().fields : [];
  }

  protected controlOf(field: IssueFormField): FormControl<FieldRaw> {
    return this.fieldControls.controls[field.fieldId];
  }

  protected error(control: AbstractControl): string | null {
    return errorKeyOf(control as FormControl<unknown>);
  }

  protected reload(): void {
    this.dialogRef.close({ reload: true });
  }

  protected async save(): Promise<void> {
    const { startDate, dueDate } = this.form.getRawValue();
    this.form.controls.dueDate.setErrors(startDate && dueDate && dueDate < startDate ? { before_start: true } : null);
    if (this.form.invalid || this.fieldControls.invalid) {
      this.form.markAllAsTouched();
      this.fieldControls.markAllAsTouched();
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    try {
      const fields = await this.changedFields();
      if (!fields) return;
      const body = this.changes(fields);
      const saved = await this.api.invoke(updateIssue, { idOrKey: this.ticket.id, 'If-Match': `"${this.ticket.version}"`, body });
      this.dialogRef.close({ saved });
    } catch (error) {
      if (isApiProblem(error) && error.status === 412) {
        this.conflict.set(true);
        return;
      }
      const controls: Record<string, AbstractControl> = { ...this.form.controls, assigneeId: this.form.controls.assignee };
      for (const [id, control] of Object.entries(this.fieldControls.controls)) controls[`fields.${id}`] = control;
      this.errorKey.set(placeApiErrors(error, controls));
    } finally {
      this.saving.set(false);
    }
  }

  /** What changed, the values as the API takes them; only the controls edited. */
  private changes(fields: Record<string, unknown>): IssueChanges {
    const controls = this.form.controls;
    const value = this.form.getRawValue();
    const body: IssueChanges = {};
    if (controls.summary.dirty) body.summary = value.summary.trim();
    if (controls.description.dirty) body.description = value.description;
    if (controls.priority.dirty) body.priority = value.priority;
    if (controls.assignee.dirty) body.assigneeId = value.assignee[0]?.id ?? null;
    if (controls.startDate.dirty) body.startDate = value.startDate ? toLocalDate(value.startDate) : null;
    if (controls.dueDate.dirty) body.dueDate = value.dueDate ? toLocalDate(value.dueDate) : null;
    if (Object.keys(fields).length > 0) body.fields = fields;
    return body;
  }

  /**
   * The custom fields edited, `null` for those cleared, ticket keys
   * resolved to ids; null if a key is unknown.
   */
  private async changedFields(): Promise<Record<string, unknown> | null> {
    const values: Record<string, unknown> = {};
    let unknown = false;
    for (const field of this.fields()) {
      const control = this.controlOf(field);
      if (!control.dirty) continue;
      let value = apiValue(field, control.value) ?? null;
      if (field.kind === 'issue_link' && value !== null) {
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
