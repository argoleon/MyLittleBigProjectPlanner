import { Component, computed, inject, input, resource, signal } from '@angular/core';
import { FormBuilder, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { Router, RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ApiConfiguration,
  AuthLayout,
  AuthService,
  errorTranslationKey,
  getCurrentTenant,
  isApiProblem,
  isErrorCode,
  isSafeReturnUrl,
  listSignInProviders,
  pathForStage,
  startOidcSignIn,
} from 'ui';

/**
 * Sign-in (mockup 07, step 1): through the tenant's identity providers, or
 * with email and password.
 */
@Component({
  selector: 'app-sign-in-page',
  imports: [
    AuthLayout,
    MatButtonModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    ReactiveFormsModule,
    RouterLink,
    TranslocoPipe,
  ],
  styleUrl: './auth-page.scss',
  templateUrl: './sign-in-page.html',
})
export class SignInPage {
  /** Page to show once signed in (query parameter). */
  readonly returnUrl = input<string>();
  /** Set after a password reset (query parameter). */
  readonly passwordChanged = input<string>();
  /** Set after the account was erased (query parameter). */
  readonly accountErased = input<string>();
  /** Error code of a sign-in through a provider that failed (query parameter). */
  readonly error = input<string>();

  private readonly auth = inject(AuthService);
  private readonly router = inject(Router);
  private readonly api = inject(Api);
  private readonly rootUrl = inject(ApiConfiguration).rootUrl;
  private readonly tenant = resource({ loader: () => this.api.invoke(getCurrentTenant) });
  private readonly providerList = resource({ loader: () => this.api.invoke(listSignInProviders) });

  /** The tenant's identity providers; none while loading or if unavailable. */
  protected readonly providers = computed(() => (this.providerList.hasValue() ? this.providerList.value() : []));

  /** Message of the error a provider sign-in came back with. */
  protected readonly returnedErrorKey = computed(() => {
    const code = this.error();
    if (!code) return null;
    return errorTranslationKey(isErrorCode(code) ? code : 'unknown');
  });

  /** Where a provider's button leads the browser (a navigation, not a call). */
  protected startUrl(providerId: string): string {
    const path = startOidcSignIn.PATH.replace('{id}', encodeURIComponent(providerId));
    const target = this.returnUrl();
    const query = isSafeReturnUrl(target) ? `?returnUrl=${encodeURIComponent(target)}` : '';
    return `${this.rootUrl}${path}${query}`;
  }

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
