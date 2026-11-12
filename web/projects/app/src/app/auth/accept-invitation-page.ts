import { Component, effect, inject, input, signal } from '@angular/core';
import { FormBuilder, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { Router, RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  AuthLayout,
  AuthService,
  InvitationPreview,
  errorTranslationKey,
  fieldErrorTranslationKey,
  isApiProblem,
  pathForStage,
} from 'ui';

import { PASSWORD_MIN_LENGTH, sameAsPassword } from './reset-password-page';

/** How the page ends once the invitation is accepted, without a session. */
type Outcome = 'external' | 'sso';

/**
 * Page of an invitation link: shows who invites to which organization, then
 * joins it. A new account chooses its name and password here; an existing
 * account joins as it is. An internal member lands in the application.
 */
@Component({
  selector: 'app-accept-invitation-page',
  imports: [
    AuthLayout,
    MatButtonModule,
    MatFormFieldModule,
    MatInputModule,
    MatProgressBarModule,
    ReactiveFormsModule,
    RouterLink,
    TranslocoPipe,
  ],
  styleUrl: './auth-page.scss',
  templateUrl: './accept-invitation-page.html',
})
export class AcceptInvitationPage {
  /** Token of the emailed link (query parameter). */
  readonly token = input<string>();

  private readonly auth = inject(AuthService);
  private readonly router = inject(Router);

  protected readonly invitation = signal<InvitationPreview | null>(null);
  protected readonly loading = signal(false);
  protected readonly submitting = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  protected readonly outcome = signal<Outcome | null>(null);

  protected readonly form = inject(FormBuilder).nonNullable.group({
    displayName: ['', Validators.required],
    password: ['', [Validators.required, Validators.minLength(PASSWORD_MIN_LENGTH)]],
    confirmation: ['', [Validators.required, sameAsPassword]],
  });

  constructor() {
    this.form.controls.password.valueChanges.subscribe(() =>
      this.form.controls.confirmation.updateValueAndValidity({ emitEvent: false }),
    );
    effect(() => {
      const token = this.token();
      if (token) void this.show(token);
    });
  }

  private async show(token: string): Promise<void> {
    this.loading.set(true);
    try {
      this.invitation.set(await this.auth.invitation(token));
    } catch (error) {
      this.errorKey.set(keyOf(error));
    } finally {
      this.loading.set(false);
    }
  }

  protected async submit(): Promise<void> {
    const token = this.token();
    const invitation = this.invitation();
    if (!token || !invitation) return;
    if (invitation.newAccount && this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.submitting.set(true);
    this.errorKey.set(null);
    try {
      const { displayName, password } = this.form.getRawValue();
      const accepted = await this.auth.acceptInvitation(
        token,
        invitation.newAccount ? { displayName, password } : undefined,
      );
      if (accepted.session) {
        await this.router.navigateByUrl(pathForStage(accepted.session.stage) ?? '/');
      } else {
        this.outcome.set(accepted.ssoRequired ? 'sso' : 'external');
      }
    } catch (error) {
      if (isApiProblem(error) && error.code === 'validation_failed' && error.fieldErrors.length > 0) {
        for (const field of error.fieldErrors) {
          const control = field.field === 'displayName' ? this.form.controls.displayName : this.form.controls.password;
          control.setErrors({ api: fieldErrorTranslationKey(field.code) });
          control.markAsTouched();
        }
      } else {
        this.errorKey.set(keyOf(error));
      }
    } finally {
      this.submitting.set(false);
    }
  }
}

function keyOf(error: unknown): string {
  return isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown');
}
