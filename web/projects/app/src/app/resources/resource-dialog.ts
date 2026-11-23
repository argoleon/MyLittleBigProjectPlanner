import { Component, computed, inject, resource, signal } from '@angular/core';
import { toSignal } from '@angular/core/rxjs-interop';
import { FormBuilder, FormControl, ReactiveFormsModule } from '@angular/forms';
import { MatAutocompleteModule, MatAutocompleteSelectedEvent } from '@angular/material/autocomplete';
import { MatButtonModule } from '@angular/material/button';
import { MatButtonToggleModule } from '@angular/material/button-toggle';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { MatSlideToggleModule } from '@angular/material/slide-toggle';
import { TranslocoPipe } from '@jsverse/transloco';
import { debounceTime, map } from 'rxjs';
import {
  Api,
  LinkableAccount,
  Resource,
  ResourceFields,
  ResourceKind,
  createResource,
  errorTranslationKey,
  isApiProblem,
  listLinkableAccounts,
  updateResource,
} from 'ui';

/** What the dialog edits: a new resource, or this one. */
export interface ResourceDialogData {
  readonly resource?: Resource;
}

/** The member a person is linked to. */
export interface LinkedAccount {
  readonly accountId: string;
  readonly displayName: string;
  readonly email?: string;
}

/** Members offered at once in the account search. */
const ACCOUNT_CHOICES = 20;

/** The body sent for the form's values. */
export function resourceBody(
  kind: ResourceKind,
  account: LinkedAccount | null,
  displayName: string,
  jobTitle: string,
  active: boolean,
): ResourceFields {
  const linked = kind === 'person' ? account : null;
  return {
    kind,
    accountId: linked?.accountId ?? null,
    displayName: linked ? null : displayName.trim(),
    jobTitle: jobTitle.trim() || null,
    active,
  };
}

/**
 * Creates or changes a resource (L07-US01): a person, linked to a member
 * or not, or a generic role; its name, job title and, once created,
 * whether it is active. Closes with the saved resource.
 */
@Component({
  selector: 'app-resource-dialog',
  imports: [
    MatAutocompleteModule,
    MatButtonModule,
    MatButtonToggleModule,
    MatDialogModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    MatSlideToggleModule,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  styleUrl: '../admin/provider-dialog.scss',
  templateUrl: './resource-dialog.html',
})
export class ResourceDialog {
  protected readonly existing = inject<ResourceDialogData>(MAT_DIALOG_DATA).resource;
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<ResourceDialog, Resource>>(MatDialogRef);
  private readonly idempotencyKey = crypto.randomUUID();

  protected readonly form = inject(FormBuilder).nonNullable.group({
    kind: [(this.existing?.kind ?? 'person') as ResourceKind],
    displayName: [this.existing && !this.existing.accountId ? this.existing.displayName : ''],
    jobTitle: [this.existing?.jobTitle ?? ''],
    active: [this.existing?.active ?? true],
  });
  protected readonly kind = toSignal(this.form.controls.kind.valueChanges, { initialValue: this.form.controls.kind.value });

  /** The member chosen, if any. */
  protected readonly account = signal<LinkedAccount | null>(
    this.existing?.accountId ? { accountId: this.existing.accountId, displayName: this.existing.displayName } : null,
  );
  protected readonly accountInput = new FormControl<string | LinkedAccount>(this.account() ?? '', { nonNullable: true });
  private readonly accountSearch = toSignal(
    this.accountInput.valueChanges.pipe(
      debounceTime(250),
      map((value) => (typeof value === 'string' ? value.trim() : '')),
    ),
    { initialValue: '' },
  );
  protected readonly linkable = resource({
    params: () => (this.kind() === 'person' ? { search: this.accountSearch() } : undefined),
    loader: ({ params }) =>
      this.api.invoke(listLinkableAccounts, { search: params.search || undefined, limit: ACCOUNT_CHOICES }),
  });
  protected readonly choices = computed<readonly LinkableAccount[]>(() => (this.linkable.hasValue() ? this.linkable.value() : []));
  /** Linked to a member: the name is the member's. */
  protected readonly linked = computed(() => this.kind() === 'person' && this.account() !== null);

  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  private readonly fieldErrors = signal<Record<string, string>>({});

  protected readonly accountName = (value: string | LinkedAccount | null): string =>
    typeof value === 'string' ? value : (value?.displayName ?? '');

  protected choose(event: MatAutocompleteSelectedEvent): void {
    this.account.set(event.option.value as LinkedAccount);
    this.fieldErrors.update(({ accountId: _, ...others }) => others);
  }

  protected clearAccount(): void {
    this.account.set(null);
    this.accountInput.setValue('');
  }

  protected fieldError(field: string): string | undefined {
    return this.fieldErrors()[field];
  }

  protected async save(): Promise<void> {
    const { kind, displayName, jobTitle, active } = this.form.getRawValue();
    const body = resourceBody(kind, this.account(), displayName, jobTitle, active);
    if (body.accountId === null && !body.displayName) {
      this.form.controls.displayName.setErrors({ required: true });
      this.form.controls.displayName.markAsTouched();
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    this.fieldErrors.set({});
    try {
      const saved = this.existing
        ? await this.api.invoke(updateResource, { id: this.existing.id, 'If-Match': `"${this.existing.version}"`, body })
        : await this.api.invoke(createResource, { 'Idempotency-Key': this.idempotencyKey, body });
      this.dialogRef.close(saved);
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
    const keys = error.fieldTranslationKeys();
    let placed = false;
    for (const field of ['displayName', 'jobTitle'] as const) {
      if (keys[field] && !(field === 'displayName' && this.linked())) {
        this.form.controls[field].setErrors({ api: keys[field] });
        this.form.controls[field].markAsTouched();
        placed = true;
      }
    }
    if (keys['accountId'] && this.kind() === 'person') {
      this.fieldErrors.set({ accountId: keys['accountId'] });
      this.accountInput.setErrors({ api: true });
      this.accountInput.markAsTouched();
      placed = true;
    }
    // A conflict (the member is linked already) says more than its field error.
    if (!placed || error.code !== 'validation_failed') this.errorKey.set(error.translationKey);
  }
}
