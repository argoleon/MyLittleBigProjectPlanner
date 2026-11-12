import { Component, inject, signal } from '@angular/core';
import { FormBuilder, ReactiveFormsModule } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatButtonToggleModule } from '@angular/material/button-toggle';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatSelectModule } from '@angular/material/select';
import { MatSlideToggleModule } from '@angular/material/slide-toggle';
import { TranslocoPipe } from '@jsverse/transloco';
import { AccountType, Api, Member, TenantRole, errorTranslationKey, isApiProblem, updateMember } from 'ui';

import { TENANT_ROLES } from './invite-dialog';

/**
 * Changes a member's account type, tenant role and local account flag.
 * Closes with the updated member.
 */
@Component({
  selector: 'app-member-dialog',
  imports: [
    MatButtonModule,
    MatButtonToggleModule,
    MatDialogModule,
    MatFormFieldModule,
    MatSelectModule,
    MatSlideToggleModule,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  styleUrl: './provider-dialog.scss',
  template: `
    <h2 mat-dialog-title>{{ 'admin.member.title' | transloco: { name: member.displayName } }}</h2>
    <form [formGroup]="form" (ngSubmit)="save()" novalidate>
      <mat-dialog-content class="fields">
        @if (errorKey(); as key) {
          <p class="message error" role="alert">{{ key | transloco }}</p>
        }
        <p class="choice-hint">{{ member.email }}</p>
        <div class="choice">
          <span class="choice-label" id="member-type-label">{{ 'admin.invite.accountType' | transloco }}</span>
          <mat-button-toggle-group formControlName="accountType" aria-labelledby="member-type-label" hideSingleSelectionIndicator>
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
        <mat-slide-toggle formControlName="localAccount" aria-describedby="local-account-hint">
          {{ 'admin.member.localAccount' | transloco }}
        </mat-slide-toggle>
        <p class="choice-hint" id="local-account-hint">{{ 'admin.member.localAccountHint' | transloco }}</p>
      </mat-dialog-content>
      <mat-dialog-actions align="end">
        <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
        <button matButton="filled" type="submit" [disabled]="saving() || form.pristine">{{ 'admin.member.save' | transloco }}</button>
      </mat-dialog-actions>
    </form>
  `,
})
export class MemberDialog {
  protected readonly member = inject<Member>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<MemberDialog, Member>>(MatDialogRef);

  protected readonly roles = TENANT_ROLES;
  protected readonly form = inject(FormBuilder).nonNullable.group({
    accountType: [this.member.accountType as AccountType],
    tenantRole: [{ value: this.member.tenantRole as TenantRole, disabled: this.member.accountType === 'external' }],
    localAccount: [this.member.localAccount],
  });
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  constructor() {
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

  protected async save(): Promise<void> {
    this.saving.set(true);
    this.errorKey.set(null);
    try {
      const member = await this.api.invoke(updateMember, {
        accountId: this.member.accountId,
        'If-Match': `"${this.member.version}"`,
        body: this.form.getRawValue(),
      });
      this.dialogRef.close(member);
    } catch (error) {
      this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
    } finally {
      this.saving.set(false);
    }
  }
}
