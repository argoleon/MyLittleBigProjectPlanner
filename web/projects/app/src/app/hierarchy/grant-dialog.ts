import { Component, inject, resource, signal } from '@angular/core';
import { takeUntilDestroyed, toObservable, toSignal } from '@angular/core/rxjs-interop';
import { AbstractControl, FormControl, FormGroup, ReactiveFormsModule, ValidationErrors, Validators } from '@angular/forms';
import { MatAutocompleteModule } from '@angular/material/autocomplete';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { MatSelectModule } from '@angular/material/select';
import { TranslocoPipe } from '@jsverse/transloco';
import { debounceTime } from 'rxjs';
import {
  AccessRole,
  Api,
  GrantCandidate,
  LocaleService,
  NodeAccess,
  errorTranslationKey,
  fieldErrorTranslationKey,
  grantNodeRole,
  isApiProblem,
  listNodeGrantCandidates,
  listNodeGrantableRoles,
} from 'ui';

import { roleLabel } from '../admin/role-draft';
import { HOLDER_ICONS } from './node-access';

/** The node a role is granted on. */
export interface GrantDialogData {
  readonly node: { readonly id: string; readonly name: string };
}

/** What the holder field holds: the text typed, then the candidate chosen. */
type HolderValue = GrantCandidate | string;

function chosen(value: HolderValue): GrantCandidate | null {
  return typeof value === 'string' ? null : value;
}

/** A member or a group must be chosen in the list, not only typed. */
function holderChosen(control: AbstractControl<HolderValue>): ValidationErrors | null {
  return chosen(control.value) ? null : { choose: true };
}

/**
 * Grants a role on a node to a member or a group (mockup 14, "Add"): a
 * search among the tenant's groups and members, and the roles the user may
 * grant there. Closes with the grant made.
 */
@Component({
  selector: 'app-grant-dialog',
  imports: [
    MatAutocompleteModule,
    MatButtonModule,
    MatDialogModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    MatSelectModule,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  templateUrl: './grant-dialog.html',
  styleUrl: './grant-dialog.scss',
})
export class GrantDialog {
  protected readonly data = inject<GrantDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<GrantDialog, NodeAccess>>(MatDialogRef);
  private readonly locale = inject(LocaleService);

  protected readonly icons = HOLDER_ICONS;
  /** The role's label in the language of the interface. */
  protected readonly roleName = (role: AccessRole): string => roleLabel(role, this.locale.language());
  protected readonly form = new FormGroup({
    holder: new FormControl<HolderValue>('', { nonNullable: true, validators: holderChosen }),
    roleId: new FormControl('', { nonNullable: true, validators: Validators.required }),
  });

  private readonly query = signal('');
  private readonly debounced = toSignal(toObservable(this.query).pipe(debounceTime(250)), { initialValue: '' });
  protected readonly found = resource({
    params: () => ({ id: this.data.node.id, search: this.debounced().trim() || undefined }),
    loader: ({ params }) => this.api.invoke(listNodeGrantCandidates, params),
  });
  protected readonly roles = resource({
    params: () => this.data.node.id,
    loader: ({ params }) => this.api.invoke(listNodeGrantableRoles, { id: params }),
  });
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  constructor() {
    // Typing searches; choosing a candidate keeps the search as it is.
    this.form.controls.holder.valueChanges.pipe(takeUntilDestroyed()).subscribe((value) => {
      if (typeof value === 'string') this.query.set(value);
    });
  }

  /** The text of the field for a candidate chosen, or for what is typed. */
  protected readonly nameOf = (value: HolderValue | null): string =>
    typeof value === 'string' ? value : (value?.name ?? '');

  protected async grant(): Promise<void> {
    const holder = chosen(this.form.controls.holder.value);
    if (this.form.invalid || !holder) {
      this.form.markAllAsTouched();
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    const roleId = this.form.controls.roleId.value;
    try {
      const granted = await this.api.invoke(grantNodeRole, {
        id: this.data.node.id,
        body: holder.kind === 'group' ? { roleId, groupId: holder.id } : { roleId, accountId: holder.id },
      });
      this.dialogRef.close(granted);
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
      const control = fieldError.field === 'roleId' ? this.form.controls.roleId : this.form.controls.holder;
      control.setErrors({ api: fieldErrorTranslationKey(fieldError.code) });
      control.markAsTouched();
      onField = true;
    }
    if (!onField) this.errorKey.set(error.translationKey);
  }
}
