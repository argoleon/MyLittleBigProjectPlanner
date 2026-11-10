import { Component, inject, input, resource, signal } from '@angular/core';
import { FormBuilder, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { Router, RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  AuthLayout,
  AuthService,
  errorTranslationKey,
  getCurrentTenant,
  isApiProblem,
  isSafeReturnUrl,
  pathForStage,
} from 'ui';

/** Sign-in with email and password (mockup 07, step 1). */
@Component({
  selector: 'app-sign-in-page',
  imports: [
    AuthLayout,
    MatButtonModule,
    MatFormFieldModule,
    MatInputModule,
    ReactiveFormsModule,
    RouterLink,
    TranslocoPipe,
  ],
  styleUrl: './auth-page.scss',
  template: `
    <ui-auth-layout>
      <h1 class="title">{{ 'auth.signIn.title' | transloco }}</h1>
      <p class="subtitle">
        @if (tenantName(); as name) {
          {{ 'auth.signIn.organization' | transloco: { name } }}
        }
      </p>

      @if (passwordChanged()) {
        <p class="message info" role="status">{{ 'auth.signIn.passwordChanged' | transloco }}</p>
      }
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
        <mat-form-field class="field" appearance="outline">
          <mat-label>{{ 'auth.signIn.password' | transloco }}</mat-label>
          <input matInput type="password" formControlName="password" autocomplete="current-password" required />
          <mat-error>{{ 'errors.field.required' | transloco }}</mat-error>
        </mat-form-field>
        <a class="link" routerLink="/forgot-password">{{ 'auth.signIn.forgotPassword' | transloco }}</a>
        <button class="submit" matButton="filled" type="submit" [disabled]="submitting()">
          {{ 'auth.signIn.submit' | transloco }}
        </button>
      </form>
    </ui-auth-layout>
  `,
})
export class SignInPage {
  /** Page to show once signed in (query parameter). */
  readonly returnUrl = input<string>();
  /** Set after a password reset (query parameter). */
  readonly passwordChanged = input<string>();

  private readonly auth = inject(AuthService);
  private readonly router = inject(Router);
  private readonly api = inject(Api);
  private readonly tenant = resource({ loader: () => this.api.invoke(getCurrentTenant) });

  protected readonly form = inject(FormBuilder).nonNullable.group({
    email: ['', [Validators.required, Validators.email]],
    password: ['', Validators.required],
  });
  protected readonly submitting = signal(false);
  /** Translation key of the last failure. */
  protected readonly errorKey = signal<string | null>(null);

  protected tenantName(): string | null {
    return this.tenant.hasValue() ? this.tenant.value().name : null;
  }

  protected async submit(): Promise<void> {
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.submitting.set(true);
    this.errorKey.set(null);
    const { email, password } = this.form.getRawValue();
    try {
      const session = await this.auth.signIn(email, password);
      const target = this.returnUrl();
      const safeTarget = isSafeReturnUrl(target) ? target : undefined;
      // Two-step verification next, when required; it comes back to the target.
      const next = pathForStage(session.stage);
      if (next) {
        await this.router.navigate([next], { queryParams: safeTarget ? { returnUrl: safeTarget } : {} });
      } else {
        await this.router.navigateByUrl(safeTarget ?? '/');
      }
    } catch (error) {
      this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
      this.form.controls.password.reset();
    } finally {
      this.submitting.set(false);
    }
  }
}
