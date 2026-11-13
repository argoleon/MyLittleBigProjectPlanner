import { Component, computed, inject, signal } from '@angular/core';
import { toSignal } from '@angular/core/rxjs-interop';
import { FormBuilder, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { DateAdapter, provideNativeDateAdapter } from '@angular/material/core';
import { MatDatepickerModule } from '@angular/material/datepicker';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { MatSelectModule } from '@angular/material/select';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  Node,
  NodeOwner,
  NodeParent,
  NodeType,
  createNode,
  errorTranslationKey,
  fieldErrorTranslationKey,
  isApiProblem,
  LocaleService,
} from 'ui';

import { OwnerPicker } from './owner-picker';
import { PROJECT_KEY, changesOf, childTypes, endsBeforeStart, normalizeKey } from './node-form';

/** Where the new node goes: under `parent`, or at the root. */
export interface NodeDialogData {
  readonly parent?: NodeParent;
}

/**
 * Creates a portfolio, program or project (mockup 05, "Add a child"): at
 * the root, or under a parent, among the types the matrix allows there.
 * Closes with the node created.
 */
@Component({
  selector: 'app-node-dialog',
  imports: [
    MatButtonModule,
    MatDatepickerModule,
    MatDialogModule,
    MatFormFieldModule,
    MatInputModule,
    MatSelectModule,
    OwnerPicker,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  // Dates are picked in the user's locale; they travel as local dates.
  providers: [provideNativeDateAdapter()],
  styleUrl: './node-dialog.scss',
  templateUrl: './node-dialog.html',
})
export class NodeDialog {
  protected readonly data = inject<NodeDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<NodeDialog, Node>>(MatDialogRef);

  protected readonly types = childTypes(this.data.parent?.type ?? null);
  protected readonly form = inject(FormBuilder).nonNullable.group({
    type: [this.types[0] as NodeType, Validators.required],
    name: ['', Validators.required],
    key: ['', [Validators.required, Validators.pattern(PROJECT_KEY)]],
    startDate: [null as Date | null],
    endDate: [null as Date | null],
  });
  protected readonly owners = signal<readonly NodeOwner[]>([]);
  private readonly type = toSignal(this.form.controls.type.valueChanges, { initialValue: this.form.controls.type.value });
  protected readonly isProject = computed(() => this.type() === 'project');
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  private readonly idempotencyKey = crypto.randomUUID();

  constructor() {
    inject(DateAdapter).setLocale(inject(LocaleService).formattingLocale());
    // Only a project has a key.
    this.form.controls.type.valueChanges.subscribe((type) => {
      const key = this.form.controls.key;
      if (type === 'project') key.enable();
      else key.disable();
    });
    if (this.form.controls.type.value !== 'project') this.form.controls.key.disable();
  }

  protected tidyKey(): void {
    const key = this.form.controls.key;
    key.setValue(normalizeKey(key.value));
  }

  protected async create(): Promise<void> {
    this.tidyKey();
    const { type, name, key, startDate, endDate } = this.form.getRawValue();
    const end = this.form.controls.endDate;
    end.setErrors(endsBeforeStart(startDate, endDate) ? { before_start: true } : null);
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    const changes = changesOf({
      name,
      description: '',
      objectives: '',
      startDate,
      endDate,
      health: 'on_track',
      owners: this.owners(),
    });
    try {
      const node = await this.api.invoke(createNode, {
        'Idempotency-Key': this.idempotencyKey,
        body: {
          type,
          name: changes.name,
          key: type === 'project' ? key : null,
          startDate: changes.startDate,
          endDate: changes.endDate,
          ownerIds: changes.ownerIds,
          parentId: this.data.parent?.id ?? null,
        },
      });
      this.dialogRef.close(node);
    } catch (error) {
      this.showError(error);
    } finally {
      this.saving.set(false);
    }
  }

  private showError(error: unknown): void {
    if (!isApiProblem(error)) {
      this.errorKey.set(errorTranslationKey('unknown'));
      return;
    }
    let onField = false;
    for (const fieldError of error.fieldErrors) {
      const control = { name: this.form.controls.name, key: this.form.controls.key }[fieldError.field as 'name' | 'key'];
      if (!control) continue;
      control.setErrors({ api: fieldErrorTranslationKey(fieldError.code) });
      control.markAsTouched();
      onField = true;
    }
    if (!onField) this.errorKey.set(error.translationKey);
  }
}
