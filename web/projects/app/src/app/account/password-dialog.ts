import { Component, inject, signal } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { AbstractControl, FormBuilder, ReactiveFormsModule, ValidationErrors, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, changeMyPassword, errorTranslationKey, fieldErrorTranslationKey, isApiProblem } from 'ui';

/** Shortest password the API accepts (`domain::account::PASSWORD_MIN_CHARS`). */
export const PASSWORD_MIN_CHARS = 12;

/** The confirmation must repeat its sibling `newPassword`. */
export function repeatsNewPassword(confirmation: AbstractControl): ValidationErrors | null {
  const newPassword = confirmation.parent?.get('newPassword')?.value as string | undefined;
  return confirmation.value && confirmation.value !== newPassword ? { mismatch: true } : null;
}

/**
 * Changes the password with the current one. The other sessions of the
 * account end; this one stays. Closes with `true` once changed.
 */
@Component({
  selector: 'app-password-dialog',
  imports: [MatButtonModule, MatDialogModule, MatFormFieldModule, MatInputModule, ReactiveFormsModule, TranslocoPipe],
  styleUrl: '../admin/provider-dialog.scss',
  template: `
    <h2 mat-dialog-title>{{ 'account.password.title' | transloco }}</h2>
    <form [formGroup]="form" (ngSubmit)="save()" novalidate>
      <mat-dialog-content class="fields">
        <p>{{ 'account.password.intro' | transloco }}</p>
        @if (errorKey(); as key) {
          <p class="message error" role="alert">{{ key | transloco }}</p>
        }
        <mat-form-field appearance="outline">
          <mat-label>{{ 'account.password.current' | transloco }}</mat-label>
          <input matInput type="password" formControlName="currentPassword" autocomplete="current-password" required />
          @if (form.controls.currentPassword.getError('api'); as key) {
            <mat-error>{{ key | transloco }}</mat-error>
          } @else {
            <mat-error>{{ 'errors.field.required' | transloco }}</mat-error>
          }
        </mat-form-field>
        <mat-form-field appearance="outline">
          <mat-label>{{ 'account.password.new' | transloco }}</mat-label>
          <input matInput type="password" formControlName="newPassword" autocomplete="new-password" required />
          <mat-hint>{{ 'account.password.newHint' | transloco: { count: minChars } }}</mat-hint>
          @if (form.controls.newPassword.getError('api'); as key) {
            <mat-error>{{ key | transloco }}</mat-error>
          } @else if (form.controls.newPassword.hasError('minlength')) {
            <mat-error>{{ 'errors.field.too_short' | transloco }}</mat-error>
          } @else {
            <mat-error>{{ 'errors.field.required' | transloco }}</mat-error>
          }
        </mat-form-field>
        <mat-form-field appearance="outline">
          <mat-label>{{ 'account.password.confirmation' | transloco }}</mat-label>
          <input matInput type="password" formControlName="confirmation" autocomplete="new-password" required />
          @if (form.controls.confirmation.hasError('mismatch')) {
            <mat-error>{{ 'account.password.mismatch' | transloco }}</mat-error>
          } @else {
            <mat-error>{{ 'errors.field.required' | transloco }}</mat-error>
          }
        </mat-form-field>
      </mat-dialog-content>
      <mat-dialog-actions align="end">
        <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
        <button matButton="filled" type="submit" [disabled]="saving()">{{ 'account.password.submit' | transloco }}</button>
      </mat-dialog-actions>
    </form>
  `,
})
export class PasswordDialog {
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<PasswordDialog, boolean>>(MatDialogRef);

  protected readonly minChars = PASSWORD_MIN_CHARS;
  protected readonly form = inject(FormBuilder).nonNullable.group({
    currentPassword: ['', Validators.required],
    newPassword: ['', [Validators.required, Validators.minLength(PASSWORD_MIN_CHARS)]],
    confirmation: ['', [Validators.required, repeatsNewPassword]],
  });
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  constructor() {
    // A new password changes whether the confirmation repeats it.
    this.form.controls.newPassword.valueChanges.pipe(takeUntilDestroyed()).subscribe(() =>
      this.form.controls.confirmation.updateValueAndValidity(),
    );
  }

  protected async save(): Promise<void> {
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    const { currentPassword, newPassword } = this.form.getRawValue();
    try {
      await this.api.invoke(changeMyPassword, { body: { currentPassword, newPassword } });
      this.dialogRef.close(true);
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
      const control = fieldError.field === 'currentPassword' || fieldError.field === 'newPassword'
        ? this.form.controls[fieldError.field]
        : null;
      if (!control) continue;
      const key =
        fieldError.field === 'currentPassword' && fieldError.code === 'incorrect'
          ? 'account.password.incorrect'
          : fieldErrorTranslationKey(fieldError.code);
      control.setErrors({ api: key });
      control.markAsTouched();
      onField = true;
    }
    if (!onField) this.errorKey.set(error.translationKey);
  }
}
