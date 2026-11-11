import { Component, inject, input, signal } from '@angular/core';
import { AbstractControl, FormBuilder, ReactiveFormsModule, ValidationErrors, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { Router, RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import { AuthLayout, AuthService, errorTranslationKey, fieldErrorTranslationKey, isApiProblem } from 'ui';

/** Shortest password the API accepts (domain rule `PASSWORD_MIN_CHARS`). */
export const PASSWORD_MIN_LENGTH = 12;

/** The confirmation must repeat the password. */
export function sameAsPassword(control: AbstractControl): ValidationErrors | null {
  const password = control.parent?.get('password')?.value as string | undefined;
  return control.value === password ? null : { mismatch: true };
}

/** Chooses a new password with the token of an emailed link. */
@Component({
  selector: 'app-reset-password-page',
  imports: [AuthLayout, MatButtonModule, MatFormFieldModule, MatInputModule, ReactiveFormsModule, RouterLink, TranslocoPipe],
  styleUrl: './auth-page.scss',
  template: `
    <ui-auth-layout>
      <h1 class="title">{{ 'auth.resetPassword.title' | transloco }}</h1>

      @if (!token()) {
        <p class="message error" role="alert">{{ 'auth.resetPassword.missingToken' | transloco }}</p>
        <a class="link" routerLink="/forgot-password">{{ 'auth.resetPassword.requestNewLink' | transloco }}</a>
      } @else {
        <p class="subtitle">{{ 'auth.resetPassword.intro' | transloco }}</p>
        @if (errorKey(); as key) {
          <p class="message error" role="alert">
            {{ key | transloco }}
            @if (tokenRejected()) {
              <a class="link" routerLink="/forgot-password">{{ 'auth.resetPassword.requestNewLink' | transloco }}</a>
            }
          </p>
        }
        <form class="form" [formGroup]="form" (ngSubmit)="submit()" novalidate>
          <mat-form-field class="field" appearance="outline">
            <mat-label>{{ 'auth.resetPassword.password' | transloco }}</mat-label>
            <input matInput type="password" formControlName="password" autocomplete="new-password" required />
            @if (form.controls.password.hasError('required')) {
              <mat-error>{{ 'errors.field.required' | transloco }}</mat-error>
            } @else if (form.controls.password.hasError('minlength')) {
              <mat-error>{{ 'errors.field.too_short' | transloco }}</mat-error>
            } @else if (form.controls.password.getError('api'); as key) {
              <mat-error>{{ key | transloco }}</mat-error>
            }
          </mat-form-field>
          <mat-form-field class="field" appearance="outline">
            <mat-label>{{ 'auth.resetPassword.confirmation' | transloco }}</mat-label>
            <input matInput type="password" formControlName="confirmation" autocomplete="new-password" required />
            <mat-error>{{ 'auth.resetPassword.mismatch' | transloco }}</mat-error>
          </mat-form-field>
          <button class="submit" matButton="filled" type="submit" [disabled]="submitting()">
            {{ 'auth.resetPassword.submit' | transloco }}
          </button>
        </form>
      }
    </ui-auth-layout>
  `,
})
export class ResetPasswordPage {
  /** Token of the emailed link (query parameter). */
  readonly token = input<string>();

  private readonly auth = inject(AuthService);
  private readonly router = inject(Router);

  protected readonly form = inject(FormBuilder).nonNullable.group({
    password: ['', [Validators.required, Validators.minLength(PASSWORD_MIN_LENGTH)]],
    confirmation: ['', [Validators.required, sameAsPassword]],
  });
  protected readonly submitting = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  /** The link itself was refused: offer a new one. */
  protected readonly tokenRejected = signal(false);

  constructor() {
    // A change of the password re-checks its confirmation.
    this.form.controls.password.valueChanges.subscribe(() =>
      this.form.controls.confirmation.updateValueAndValidity({ emitEvent: false }),
    );
  }

  protected async submit(): Promise<void> {
    const token = this.token();
    if (!token || this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.submitting.set(true);
    this.errorKey.set(null);
    this.tokenRejected.set(false);
    try {
      await this.auth.confirmPasswordReset(token, this.form.getRawValue().password);
      await this.router.navigate(['/sign-in'], { queryParams: { passwordChanged: 1 } });
    } catch (error) {
      if (!isApiProblem(error)) {
        this.errorKey.set(errorTranslationKey('unknown'));
      } else if (error.code === 'validation_failed' && error.fieldErrors.length > 0) {
        const password = this.form.controls.password;
        password.setErrors({ api: fieldErrorTranslationKey(error.fieldErrors[0].code) });
        password.markAsTouched();
      } else {
        this.errorKey.set(error.translationKey);
        this.tokenRejected.set(error.translationKey === 'errors.auth.reset_token_invalid');
      }
    } finally {
      this.submitting.set(false);
    }
  }
}
