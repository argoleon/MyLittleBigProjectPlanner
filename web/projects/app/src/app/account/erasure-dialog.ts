import { Component, inject, signal } from '@angular/core';
import { AbstractControl, FormBuilder, ReactiveFormsModule, ValidationErrors, ValidatorFn, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatCheckboxModule } from '@angular/material/checkbox';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, Me, eraseMyAccount, errorTranslationKey, fieldErrorTranslationKey, isApiProblem } from 'ui';

/** Whether `typed` is `email`, whatever its case and surrounding spaces. */
export function confirmsEmail(typed: string, email: string): boolean {
  return typed.trim() !== '' && typed.trim().toLowerCase() === email.trim().toLowerCase();
}

/** The control must hold `email` (see {@link confirmsEmail}). */
export function emailConfirmation(email: string): ValidatorFn {
  return (control: AbstractControl): ValidationErrors | null =>
    !control.value || confirmsEmail(control.value as string, email) ? null : { mismatch: true };
}

/**
 * Erasure of the signed-in account (right to erasure, L02-US09): says what
 * goes and what stays, asks for the password (or, for an account without
 * one, relies on a recent sign-in through the organization) and the email,
 * typed. Closes with `true` once erased: every session has ended.
 */
@Component({
  selector: 'app-erasure-dialog',
  imports: [
    MatButtonModule,
    MatCheckboxModule,
    MatDialogModule,
    MatFormFieldModule,
    MatInputModule,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  styleUrl: './erasure-dialog.scss',
  template: `
    <h2 mat-dialog-title>{{ 'account.erasure.title' | transloco }}</h2>
    <form [formGroup]="form" (ngSubmit)="erase()" novalidate>
      <mat-dialog-content class="fields">
        <p>{{ 'account.erasure.intro' | transloco }}</p>
        <ul class="consequences">
          <li>{{ 'account.erasure.removed' | transloco }}</li>
          <li>{{ 'account.erasure.pseudonym' | transloco }}</li>
          <li>{{ 'account.erasure.kept' | transloco }}</li>
          <li>{{ 'account.erasure.signedOut' | transloco }}</li>
        </ul>
        <p class="irreversible">{{ 'account.erasure.irreversible' | transloco }}</p>
        @if (errorKey(); as key) {
          <p class="message error" role="alert">{{ key | transloco }}</p>
        }
        @if (me.hasPassword) {
          <mat-form-field appearance="outline">
            <mat-label>{{ 'account.erasure.password' | transloco }}</mat-label>
            <input matInput type="password" formControlName="password" autocomplete="current-password" required />
            @if (form.controls.password.getError('api'); as key) {
              <mat-error>{{ key | transloco }}</mat-error>
            } @else {
              <mat-error>{{ 'errors.field.required' | transloco }}</mat-error>
            }
          </mat-form-field>
        } @else {
          <p class="hint">{{ 'account.erasure.ssoHint' | transloco }}</p>
        }
        <mat-form-field appearance="outline">
          <mat-label>{{ 'account.erasure.confirmation' | transloco: { email: me.email } }}</mat-label>
          <input matInput formControlName="confirmation" autocomplete="off" required />
          @if (form.controls.confirmation.hasError('mismatch')) {
            <mat-error>{{ 'account.erasure.mismatch' | transloco }}</mat-error>
          } @else {
            <mat-error>{{ 'errors.field.required' | transloco }}</mat-error>
          }
        </mat-form-field>
        <mat-checkbox formControlName="purgeComments">{{ 'account.erasure.purgeComments' | transloco }}</mat-checkbox>
      </mat-dialog-content>
      <mat-dialog-actions align="end">
        <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
        <button matButton="filled" type="submit" class="confirm" [disabled]="erasing()">
          {{ 'account.erasure.submit' | transloco }}
        </button>
      </mat-dialog-actions>
    </form>
  `,
})
export class ErasureDialog {
  protected readonly me = inject<Me>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<ErasureDialog, boolean>>(MatDialogRef);

  protected readonly form = inject(FormBuilder).nonNullable.group({
    password: ['', this.me.hasPassword ? Validators.required : []],
    confirmation: ['', [Validators.required, emailConfirmation(this.me.email)]],
    purgeComments: [false],
  });
  protected readonly erasing = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  protected async erase(): Promise<void> {
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.erasing.set(true);
    this.errorKey.set(null);
    const { password, confirmation, purgeComments } = this.form.getRawValue();
    try {
      await this.api.invoke(eraseMyAccount, {
        body: { confirmation, purgeComments, ...(this.me.hasPassword ? { password } : {}) },
      });
      this.dialogRef.close(true);
    } catch (error) {
      this.showError(error);
    } finally {
      this.erasing.set(false);
    }
  }

  private showError(error: unknown): void {
    if (!isApiProblem(error)) {
      this.errorKey.set(errorTranslationKey('unknown'));
      return;
    }
    let onField = false;
    for (const fieldError of error.fieldErrors) {
      if (fieldError.field !== 'password' && fieldError.field !== 'confirmation') continue;
      const control = this.form.controls[fieldError.field];
      const key =
        fieldError.field === 'password' && fieldError.code === 'incorrect'
          ? 'account.password.incorrect'
          : fieldError.code === 'mismatch'
            ? 'account.erasure.mismatch'
            : fieldErrorTranslationKey(fieldError.code);
      control.setErrors(fieldError.code === 'mismatch' ? { mismatch: true } : { api: key });
      control.markAsTouched();
      onField = true;
    }
    if (!onField) this.errorKey.set(error.translationKey);
  }
}
