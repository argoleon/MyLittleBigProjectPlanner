import { Component, inject, input } from '@angular/core';
import { Router } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import { AuthLayout, isSafeReturnUrl } from 'ui';

import { MfaEnrollment } from './mfa-enrollment';

/**
 * Enrollment the tenant requires after the password: the member enables
 * two-step verification before reaching anything else.
 */
@Component({
  selector: 'app-enroll-page',
  imports: [AuthLayout, MfaEnrollment, TranslocoPipe],
  styleUrl: './auth-page.scss',
  template: `
    <ui-auth-layout>
      <h1 class="title">{{ 'auth.enroll.title' | transloco }}</h1>
      <p class="subtitle">{{ 'auth.enroll.intro' | transloco }}</p>
      <app-mfa-enrollment (finished)="continue()" />
    </ui-auth-layout>
  `,
})
export class EnrollPage {
  /** Page to show once signed in (query parameter). */
  readonly returnUrl = input<string>();

  private readonly router = inject(Router);

  protected async continue(): Promise<void> {
    const target = this.returnUrl();
    await this.router.navigateByUrl(isSafeReturnUrl(target) ? target : '/');
  }
}
