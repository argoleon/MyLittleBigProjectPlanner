import { Clipboard } from '@angular/cdk/clipboard';
import { Component, OnInit, computed, inject, output, signal } from '@angular/core';
import { FormBuilder, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { TranslocoPipe, TranslocoService } from '@jsverse/transloco';
import { Api, AuthService, errorTranslationKey, isApiProblem, setUpMfa } from 'ui';

import {
  EnrollmentEvent,
  INITIAL_ENROLLMENT,
  backupCodesFile,
  enrollmentReducer,
  groupedSecret,
  qrCodeDataUrl,
} from './mfa-enrollment.state';

/**
 * Enabling two-step verification, step by step (see `mfa-enrollment.state.ts`):
 * scan the QR code (or type the key), confirm with a code, keep the backup
 * codes. Used by the enrollment page and the security dialog.
 */
@Component({
  selector: 'app-mfa-enrollment',
  imports: [
    MatButtonModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    MatProgressBarModule,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  styleUrl: './mfa-enrollment.scss',
  templateUrl: './mfa-enrollment.html',
})
export class MfaEnrollment implements OnInit {
  /** The member kept the backup codes: enrollment is over. */
  readonly finished = output<void>();

  private readonly api = inject(Api);
  private readonly auth = inject(AuthService);
  private readonly clipboard = inject(Clipboard);
  private readonly transloco = inject(TranslocoService);

  protected readonly state = signal(INITIAL_ENROLLMENT);
  protected readonly copied = signal(false);
  protected readonly form = inject(FormBuilder).nonNullable.group({
    code: ['', [Validators.required, Validators.pattern(/^\s*\d{3}\s?\d{3}\s*$/)]],
  });

  protected readonly qrCode = computed(() => {
    const state = this.state();
    return state.step === 'scan' ? qrCodeDataUrl(state.setup) : '';
  });
  protected readonly secret = computed(() => {
    const state = this.state();
    return state.step === 'scan' ? groupedSecret(state.setup.secret) : '';
  });
  protected readonly confirming = computed(() => {
    const state = this.state();
    return state.step === 'scan' && state.confirming;
  });
  protected readonly errorKey = computed(() => {
    const state = this.state();
    return state.step === 'scan' || state.step === 'failed' ? state.errorKey : null;
  });
  protected readonly backupCodes = computed(() => {
    const state = this.state();
    return state.step === 'codes' ? state.backupCodes : [];
  });

  ngOnInit(): void {
    void this.start();
  }

  protected async start(): Promise<void> {
    this.dispatch({ type: 'restart' });
    try {
      this.dispatch({ type: 'setupLoaded', setup: await this.api.invoke(setUpMfa) });
    } catch (error) {
      this.dispatch({ type: 'setupFailed', errorKey: keyOf(error) });
    }
  }

  protected async confirm(): Promise<void> {
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.dispatch({ type: 'confirming' });
    try {
      const enabled = await this.auth.confirmMfa(this.form.getRawValue().code);
      this.dispatch({ type: 'confirmed', backupCodes: enabled.backupCodes });
    } catch (error) {
      this.dispatch({ type: 'confirmFailed', errorKey: keyOf(error) });
      this.form.reset();
    }
  }

  protected copy(): void {
    this.copied.set(this.clipboard.copy(this.backupCodes().join('\n')));
  }

  protected download(): void {
    const text = backupCodesFile(this.backupCodes(), this.transloco.translate('auth.mfa.fileTitle'));
    const url = URL.createObjectURL(new Blob([text], { type: 'text/plain' }));
    const link = document.createElement('a');
    link.href = url;
    link.download = 'backup-codes.txt';
    link.click();
    URL.revokeObjectURL(url);
  }

  private dispatch(event: EnrollmentEvent): void {
    this.state.update((state) => enrollmentReducer(state, event));
  }
}

function keyOf(error: unknown): string {
  return isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown');
}
