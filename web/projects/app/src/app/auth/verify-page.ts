import { Component, inject, input, signal } from '@angular/core';
import { FormBuilder, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { Router } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import { AuthLayout, AuthService, errorTranslationKey, isApiProblem, isSafeReturnUrl } from 'ui';

/** Second step of sign-in: a code from the authenticator app, or a backup code (mockup 07). */
@Component({
  selector: 'app-verify-page',
  imports: [AuthLayout, MatButtonModule, MatFormFieldModule, MatInputModule, ReactiveFormsModule, TranslocoPipe],
  styleUrl: './auth-page.scss',
  template: `
    <ui-auth-layout>
      <p class="eyebrow">{{ 'auth.verify.step' | transloco }}</p>
      <h1 class="title">{{ 'auth.verify.title' | transloco }}</h1>
      <p class="subtitle">
        {{ (useBackupCode() ? 'auth.verify.backupIntro' : 'auth.verify.intro') | transloco }}
      </p>
      @if (errorKey(); as key) {
        <p class="message error" role="alert">{{ key | transloco }}</p>
      }
      <form class="form" [formGroup]="form" (ngSubmit)="submit()" novalidate>
        <!-- One label and one input (control flow inside mat-form-field breaks their link). -->
        <mat-form-field class="field" appearance="outline">
          <mat-label>{{ (useBackupCode() ? 'auth.verify.backupCode' : 'auth.mfa.code') | transloco }}</mat-label>
          <input
            matInput
            formControlName="code"
            [attr.inputmode]="useBackupCode() ? null : 'numeric'"
            [attr.autocomplete]="useBackupCode() ? 'off' : 'one-time-code'"
            [attr.maxlength]="useBackupCode() ? 11 : 7"
            autocapitalize="none"
            required
          />
          <mat-error>{{ 'errors.field.required' | transloco }}</mat-error>
        </mat-form-field>
        <button class="submit" matButton="filled" type="submit" [disabled]="submitting()">
          {{ 'auth.verify.submit' | transloco }}
        </button>
        <button class="switch" matButton type="button" (click)="toggleBackupCode()">
          {{ (useBackupCode() ? 'auth.verify.useApp' : 'auth.verify.useBackup') | transloco }}
        </button>
      </form>
    </ui-auth-layout>
  `,
})
export class VerifyPage {
  /** Page to show once signed in (query parameter). */
  readonly returnUrl = input<string>();

  private readonly auth = inject(AuthService);
  private readonly router = inject(Router);

  protected readonly form = inject(FormBuilder).nonNullable.group({
    code: ['', Validators.required],
  });
  protected readonly useBackupCode = signal(false);
  protected readonly submitting = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  protected toggleBackupCode(): void {
    this.useBackupCode.update((backup) => !backup);
    this.form.reset();
    this.errorKey.set(null);
  }

  protected async submit(): Promise<void> {
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.submitting.set(true);
    this.errorKey.set(null);
    try {
      await this.auth.verifySignIn(this.form.getRawValue().code);
      const target = this.returnUrl();
      await this.router.navigateByUrl(isSafeReturnUrl(target) ? target : '/');
    } catch (error) {
      this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
      this.form.reset();
    } finally {
      this.submitting.set(false);
    }
  }
}
