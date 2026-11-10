import { Component, inject, signal } from '@angular/core';
import { FormBuilder, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import { AuthLayout, AuthService, errorTranslationKey, isApiProblem } from 'ui';

/**
 * Asks for a password reset link. The answer is the same whether the email
 * has an account or not.
 */
@Component({
  selector: 'app-forgot-password-page',
  imports: [AuthLayout, MatButtonModule, MatFormFieldModule, MatInputModule, ReactiveFormsModule, RouterLink, TranslocoPipe],
  styleUrl: './auth-page.scss',
  template: `
    <ui-auth-layout>
      <h1 class="title">{{ 'auth.forgotPassword.title' | transloco }}</h1>

      @if (sentTo(); as email) {
        <p class="message info" role="status">{{ 'auth.forgotPassword.sent' | transloco: { email } }}</p>
      } @else {
        <p class="subtitle">{{ 'auth.forgotPassword.intro' | transloco }}</p>
        @if (errorKey(); as key) {
          <p class="message error" role="alert">{{ key | transloco }}</p>
        }
        <form class="form" [formGroup]="form" (ngSubmit)="submit()" novalidate>
          <mat-form-field class="field" appearance="outline">
            <mat-label>{{ 'auth.signIn.email' | transloco }}</mat-label>
            <input matInput type="email" formControlName="email" autocomplete="username" required />
            @if (form.controls.email.hasError('required')) {
              <mat-error>{{ 'errors.field.required' | transloco }}</mat-error>
            } @else if (form.controls.email.hasError('email')) {
              <mat-error>{{ 'errors.field.email' | transloco }}</mat-error>
            }
          </mat-form-field>
          <button class="submit" matButton="filled" type="submit" [disabled]="submitting()">
            {{ 'auth.forgotPassword.submit' | transloco }}
          </button>
        </form>
      }
      <p>
        <a class="link" routerLink="/sign-in">{{ 'auth.forgotPassword.backToSignIn' | transloco }}</a>
      </p>
    </ui-auth-layout>
  `,
})
export class ForgotPasswordPage {
  private readonly auth = inject(AuthService);

  protected readonly form = inject(FormBuilder).nonNullable.group({
    email: ['', [Validators.required, Validators.email]],
  });
  protected readonly submitting = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  /** The email the link was sent to, once the request is accepted. */
  protected readonly sentTo = signal<string | null>(null);

  protected async submit(): Promise<void> {
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.submitting.set(true);
    this.errorKey.set(null);
    const { email } = this.form.getRawValue();
    try {
      await this.auth.requestPasswordReset(email);
      this.sentTo.set(email);
    } catch (error) {
      this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
    } finally {
      this.submitting.set(false);
    }
  }
}
