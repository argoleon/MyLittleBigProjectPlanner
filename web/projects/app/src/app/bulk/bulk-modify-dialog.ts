import { Component, computed, effect, inject, resource, signal } from '@angular/core';
import { FormControl, FormRecord, ReactiveFormsModule } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatCheckboxModule } from '@angular/material/checkbox';
import { provideNativeDateAdapter } from '@angular/material/core';
import { MatDatepickerModule } from '@angular/material/datepicker';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatSelectModule } from '@angular/material/select';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  BulkAction,
  BulkActionFields,
  ErrorState,
  ISSUE_PRIORITY,
  IssueFormField,
  IssuePerson,
  IssuePriority,
  createBulkAction,
  errorTranslationKey,
  getBulkOptions,
  isApiProblem,
  toLocalDate,
} from 'ui';

import { FieldInput } from '../issues/field-input';
import { FieldRaw, apiValue, controlOf } from '../issues/issue-fields';
import { PeoplePicker } from '../issues/people-picker';
import { BulkDialogData } from './bulk-flow';

/** Keep a value, set a new one, or clear it. */
export type ChangeMode = 'keep' | 'set' | 'clear';

/**
 * Modifies the selected tickets (L06-US08): priority, assignee, start and
 * due dates, and the custom fields they all have; only what the member
 * chooses to change is sent, each ticket checked against its own rules.
 */
@Component({
  selector: 'app-bulk-modify-dialog',
  imports: [
    ErrorState,
    FieldInput,
    MatButtonModule,
    MatCheckboxModule,
    MatDatepickerModule,
    MatDialogModule,
    MatFormFieldModule,
    MatInputModule,
    MatProgressBarModule,
    MatSelectModule,
    PeoplePicker,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  providers: [provideNativeDateAdapter()],
  templateUrl: './bulk-modify-dialog.html',
  styleUrl: './bulk-dialog.scss',
})
export class BulkModifyDialog {
  protected readonly data = inject<BulkDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<BulkModifyDialog, BulkAction>>(MatDialogRef);

  protected readonly priorities = ISSUE_PRIORITY;
  protected readonly modes: readonly ChangeMode[] = ['keep', 'set', 'clear'];
  protected readonly options = resource({
    loader: () => this.api.invoke(getBulkOptions, { body: this.data.selection }),
  });
  protected readonly fields = computed<IssueFormField[]>(() =>
    this.options.hasValue() ? (this.options.value().fields as IssueFormField[]) : [],
  );

  protected readonly priority = new FormControl<IssuePriority | 'keep'>('keep', { nonNullable: true });
  protected readonly assigneeMode = new FormControl<ChangeMode>('keep', { nonNullable: true });
  protected readonly assignee = new FormControl<readonly IssuePerson[]>([], { nonNullable: true });
  protected readonly startMode = new FormControl<ChangeMode>('keep', { nonNullable: true });
  protected readonly startDate = new FormControl<Date | null>(null);
  protected readonly dueMode = new FormControl<ChangeMode>('keep', { nonNullable: true });
  protected readonly dueDate = new FormControl<Date | null>(null);
  /** The custom fields to change, and their values. */
  protected readonly changing = new FormRecord<FormControl<boolean>>({});
  protected readonly values = new FormRecord<FormControl<FieldRaw>>({});

  protected readonly running = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  constructor() {
    effect(() => {
      for (const field of this.fields()) {
        if (!this.values.contains(field.fieldId)) {
          this.values.addControl(field.fieldId, controlOf(field));
          this.changing.addControl(field.fieldId, new FormControl(false, { nonNullable: true }));
        }
      }
    });
  }

  /** The action as the API takes it, or `null` when nothing would change. */
  protected action(): BulkActionFields | null {
    const action: Extract<BulkActionFields, { kind: 'modify' }> = { kind: 'modify' };
    let any = false;
    if (this.priority.value !== 'keep') {
      action.priority = this.priority.value;
      any = true;
    }
    const assignee = this.assignee.value[0];
    if (this.assigneeMode.value === 'clear' || (this.assigneeMode.value === 'set' && assignee)) {
      action.assigneeId = this.assigneeMode.value === 'clear' ? null : assignee.id;
      any = true;
    }
    const date = (mode: ChangeMode, value: Date | null): string | null | undefined =>
      mode === 'clear' ? null : mode === 'set' && value ? toLocalDate(value) : undefined;
    const start = date(this.startMode.value, this.startDate.value);
    if (start !== undefined) {
      action.startDate = start;
      any = true;
    }
    const due = date(this.dueMode.value, this.dueDate.value);
    if (due !== undefined) {
      action.dueDate = due;
      any = true;
    }
    const fields: Record<string, unknown> = {};
    for (const field of this.fields()) {
      if (this.changing.controls[field.fieldId]?.value) {
        fields[field.fieldId] = apiValue(field, this.values.controls[field.fieldId]?.value) ?? null;
        any = true;
      }
    }
    if (Object.keys(fields).length > 0) action.fields = fields;
    return any ? action : null;
  }

  protected async run(): Promise<void> {
    const action = this.action();
    if (!action) {
      this.errorKey.set('bulk.modify.nothing');
      return;
    }
    this.running.set(true);
    this.errorKey.set(null);
    try {
      const done = await this.api.invoke(createBulkAction, { body: { selection: this.data.selection, action } });
      this.dialogRef.close(done);
    } catch (error) {
      this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
    } finally {
      this.running.set(false);
    }
  }
}
