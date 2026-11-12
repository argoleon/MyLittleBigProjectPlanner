import { Component, inject, signal } from '@angular/core';
import { FormBuilder, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatButtonToggleModule } from '@angular/material/button-toggle';
import { MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { MatSelectModule } from '@angular/material/select';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  AccountType,
  Api,
  Invitation,
  TenantRole,
  createInvitation,
  errorTranslationKey,
  fieldErrorTranslationKey,
  isApiProblem,
} from 'ui';

/** Tenant roles an admin can give, in display order. */
export const TENANT_ROLES: readonly TenantRole[] = ['member', 'configuration_admin', 'tenant_admin'];

/**
 * Invites someone by email, with an account type and a tenant role. An
 * external account is always a plain member (request portal only). Closes
 * with the invitation sent.
 */
@Component({
  selector: 'app-invite-dialog',
  imports: [
    MatButtonModule,
    MatButtonToggleModule,
    MatDialogModule,
    MatFormFieldModule,
    MatInputModule,
    MatSelectModule,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  styleUrl: './provider-dialog.scss',
  template: `
    <h2 mat-dialog-title>{{ 'admin.invite.title' | transloco }}</h2>
    <form [formGroup]="form" (ngSubmit)="send()" novalidate>
      <mat-dialog-content class="fields">
        @if (errorKey(); as key) {
          <p class="message error" role="alert">{{ key | transloco }}</p>
        }
        <mat-form-field appearance="outline">
          <mat-label>{{ 'admin.invite.email' | transloco }}</mat-label>
          <input matInput type="email" formControlName="email" autocomplete="off" required />
          @if (form.controls.email.getError('api'); as key) {
            <mat-error>{{ key | transloco }}</mat-error>
          } @else if (form.controls.email.hasError('required')) {
            <mat-error>{{ 'errors.field.required' | transloco }}</mat-error>
          } @else {
            <mat-error>{{ 'errors.field.email' | transloco }}</mat-error>
          }
        </mat-form-field>
        <div class="choice">
          <span class="choice-label" id="invite-type-label">{{ 'admin.invite.accountType' | transloco }}</span>
          <mat-button-toggle-group formControlName="accountType" aria-labelledby="invite-type-label" hideSingleSelectionIndicator>
            <mat-button-toggle value="internal">{{ 'member.accountType.internal' | transloco }}</mat-button-toggle>
            <mat-button-toggle value="external">{{ 'member.accountType.external' | transloco }}</mat-button-toggle>
          </mat-button-toggle-group>
          <p class="choice-hint">{{ 'admin.invite.' + form.controls.accountType.value + 'Hint' | transloco }}</p>
        </div>
        <mat-form-field appearance="outline">
          <mat-label>{{ 'admin.invite.role' | transloco }}</mat-label>
          <mat-select formControlName="tenantRole">
            @for (role of roles; track role) {
              <mat-option [value]="role">{{ 'shell.role.' + role | transloco }}</mat-option>
            }
          </mat-select>
          @if (form.controls.accountType.value === 'external') {
            <mat-hint>{{ 'admin.invite.externalRole' | transloco }}</mat-hint>
          }
        </mat-form-field>
      </mat-dialog-content>
      <mat-dialog-actions align="end">
        <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
        <button matButton="filled" type="submit" [disabled]="sending()">{{ 'admin.invite.submit' | transloco }}</button>
      </mat-dialog-actions>
    </form>
  `,
})
export class InviteDialog {
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<InviteDialog, Invitation>>(MatDialogRef);

  protected readonly roles = TENANT_ROLES;
  protected readonly form = inject(FormBuilder).nonNullable.group({
    email: ['', [Validators.required, Validators.email]],
    accountType: ['internal' as AccountType],
    tenantRole: ['member' as TenantRole],
  });
  protected readonly sending = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  /** One per dialog: sending again after a lost answer invites once. */
  private readonly idempotencyKey = crypto.randomUUID();

  constructor() {
    // An external account administers nothing.
    this.form.controls.accountType.valueChanges.subscribe((type) => {
      const role = this.form.controls.tenantRole;
      if (type === 'external') {
        role.setValue('member');
        role.disable();
      } else {
        role.enable();
      }
    });
  }

  protected async send(): Promise<void> {
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.sending.set(true);
    this.errorKey.set(null);
    try {
      const invitation = await this.api.invoke(createInvitation, {
        'Idempotency-Key': this.idempotencyKey,
        body: { ...this.form.getRawValue(), email: this.form.getRawValue().email.trim() },
      });
      this.dialogRef.close(invitation);
    } catch (error) {
      if (isApiProblem(error) && error.code === 'validation_failed' && error.fieldErrors.length > 0) {
        for (const field of error.fieldErrors) {
          if (field.field === 'email') {
            this.form.controls.email.setErrors({ api: fieldErrorTranslationKey(field.code) });
            this.form.controls.email.markAsTouched();
          } else {
            this.errorKey.set(fieldErrorTranslationKey(field.code));
          }
        }
      } else {
        this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
      }
    } finally {
      this.sending.set(false);
    }
  }
}
