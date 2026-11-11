import { Component, computed, effect, inject, input, output, signal, untracked } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { FormControl, FormGroup, ReactiveFormsModule } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { MatSlideToggleModule } from '@angular/material/slide-toggle';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ConfirmDialog,
  ConfirmDialogData,
  LANGUAGES,
  Language,
  LocaleService,
  PERMISSION,
  Permission,
  Role,
  createRole,
  deleteRole,
  errorTranslationKey,
  fieldErrorTranslationKey,
  isApiProblem,
  updateRole,
} from 'ui';

import { RoleDraft, RoleSource, bodyOf, draftOf, draftProblems, roleLabel, sameDraft } from './role-draft';

type PermissionControls = Record<Permission, FormControl<boolean>>;

/**
 * Detail panel of a node role (mockup 09): labels in French and English and
 * one switch per permission of the catalog. Saves, discards, duplicates and
 * deletes; tells whether there are unsaved changes (`dirty`).
 */
@Component({
  selector: 'app-role-editor',
  imports: [MatButtonModule, MatFormFieldModule, MatInputModule, MatSlideToggleModule, ReactiveFormsModule, TranslocoPipe],
  styleUrl: './role-editor.scss',
  templateUrl: './role-editor.html',
})
export class RoleEditor {
  /** The role to edit, or a new one. Changing it starts over. */
  readonly source = input.required<RoleSource>();
  /** The role was created or changed. */
  readonly saved = output<Role>();
  /** The role was deleted. */
  readonly deleted = output<Role>();
  /** Asks for a new role with the permissions of this one. */
  readonly duplicate = output<Role>();

  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly locale = inject(LocaleService);

  protected readonly languages = LANGUAGES;
  protected readonly catalog = PERMISSION;
  protected readonly labelKeys: Record<Language, string> = { fr: 'admin.roles.labelFr', en: 'admin.roles.labelEn' };

  protected readonly form = new FormGroup({
    fr: new FormControl('', { nonNullable: true }),
    en: new FormControl('', { nonNullable: true }),
    permissions: new FormGroup(
      Object.fromEntries(PERMISSION.map((p) => [p, new FormControl(false, { nonNullable: true })])) as PermissionControls,
    ),
  });

  /** The draft as last loaded or saved, and as currently edited. */
  private readonly baseline = signal<RoleDraft>(draftOf({ kind: 'new' }));
  private readonly current = signal<RoleDraft>(draftOf({ kind: 'new' }));
  /** Whether there are changes not saved yet. */
  readonly dirty = computed(() => !sameDraft(this.current(), this.baseline()));

  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  protected readonly permissionsError = signal(false);
  private idempotencyKey = crypto.randomUUID();

  protected readonly role = computed(() => {
    const source = this.source();
    return source.kind === 'edit' ? source.role : null;
  });

  protected readonly heading = computed((): { key: string; params?: Record<string, string> } => {
    const source = this.source();
    const language = this.locale.language();
    if (source.kind === 'edit') return { key: 'admin.roles.title', params: { label: roleLabel(source.role, language) } };
    if (source.copyOf) return { key: 'admin.roles.copyTitle', params: { label: roleLabel(source.copyOf, language) } };
    return { key: 'admin.roles.newTitle' };
  });

  constructor() {
    this.form.valueChanges.pipe(takeUntilDestroyed()).subscribe(() => {
      this.current.set(this.draft());
      if (this.current().permissions.length > 0) this.permissionsError.set(false);
    });
    effect(() => {
      const source = this.source();
      untracked(() => {
        this.idempotencyKey = crypto.randomUUID();
        this.load(draftOf(source));
      });
    });
  }

  /** Puts the form back as last loaded or saved. */
  discard(): void {
    this.load(this.baseline());
  }

  protected async save(): Promise<void> {
    const draft = this.draft();
    const problems = draftProblems(draft);
    this.permissionsError.set(problems.noPermission);
    if (problems.noLabel) {
      const control = this.form.controls[this.locale.language()];
      control.setErrors({ required: true });
      control.markAsTouched();
    }
    if (problems.noLabel || problems.noPermission) return;

    const source = this.source();
    this.saving.set(true);
    this.errorKey.set(null);
    try {
      const body = bodyOf(draft);
      const saved =
        source.kind === 'edit'
          ? await this.api.invoke(updateRole, { id: source.role.id, 'If-Match': `"${source.role.version}"`, body })
          : await this.api.invoke(createRole, { 'Idempotency-Key': this.idempotencyKey, body });
      this.load(draftOf({ kind: 'edit', role: saved }));
      this.saved.emit(saved);
    } catch (error) {
      this.showError(error);
    } finally {
      this.saving.set(false);
    }
  }

  protected remove(role: Role): void {
    const data: ConfirmDialogData = {
      titleKey: 'admin.roles.deleteTitle',
      messageKey: 'admin.roles.deleteMessage',
      confirmKey: 'admin.roles.deleteConfirm',
      params: { label: roleLabel(role, this.locale.language()) },
    };
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, { data, width: 'min(480px, 100vw)' })
      .afterClosed()
      .subscribe(async (confirmed) => {
        if (!confirmed) return;
        this.saving.set(true);
        this.errorKey.set(null);
        try {
          await this.api.invoke(deleteRole, { id: role.id, 'If-Match': `"${role.version}"` });
          this.deleted.emit(role);
        } catch (error) {
          this.showError(error);
        } finally {
          this.saving.set(false);
        }
      });
  }

  private draft(): RoleDraft {
    const value = this.form.getRawValue();
    return {
      labels: { fr: value.fr, en: value.en },
      permissions: PERMISSION.filter((p) => value.permissions[p]),
    };
  }

  private load(draft: RoleDraft): void {
    const permissions = Object.fromEntries(PERMISSION.map((p) => [p, draft.permissions.includes(p)]));
    this.form.reset({ ...draft.labels, permissions }, { emitEvent: false });
    this.baseline.set(draft);
    this.current.set(draft);
    this.errorKey.set(null);
    this.permissionsError.set(false);
  }

  private showError(error: unknown): void {
    if (!isApiProblem(error)) {
      this.errorKey.set(errorTranslationKey('unknown'));
      return;
    }
    let onField = false;
    for (const fieldError of error.fieldErrors) {
      const key = fieldErrorTranslationKey(fieldError.code);
      if (fieldError.field === 'permissions') {
        this.permissionsError.set(true);
        onField = true;
        continue;
      }
      // `labelTranslations` (taken: both labels) or `labelTranslations.<language>`.
      const [field, language] = fieldError.field.split('.');
      if (field !== 'labelTranslations') continue;
      for (const target of LANGUAGES.filter((l) => !language || l === language)) {
        const control = this.form.controls[target];
        if (!language && !control.value.trim()) continue;
        control.setErrors({ api: key });
        control.markAsTouched();
        onField = true;
      }
    }
    if (!onField) this.errorKey.set(error.translationKey);
  }
}
