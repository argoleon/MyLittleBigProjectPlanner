import { Component, inject, resource, signal } from '@angular/core';
import { FormBuilder, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, ErrorState, disableMfa, errorTranslationKey, getMfa, isApiProblem } from 'ui';

import { MfaEnrollment } from '../auth/mfa-enrollment';

/**
 * Two-step verification of the signed-in member, opened from the account
 * menu: enable it, or see it and disable it with a code. (The My account
 * screen of L02-US08 reuses it.)
 */
@Component({
  selector: 'app-mfa-dialog',
  imports: [
    ErrorState,
    MatButtonModule,
    MatDialogModule,
    MatFormFieldModule,
    MatInputModule,
    MatProgressBarModule,
    MfaEnrollment,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  styleUrl: '../auth/auth-page.scss',
  template: `
    <h2 mat-dialog-title>{{ 'auth.mfa.title' | transloco }}</h2>
    <mat-dialog-content>
      @if (status.isLoading()) {
        <mat-progress-bar mode="indeterminate" [attr.aria-label]="'state.loading' | transloco" />
      } @else if (status.error()) {
        <ui-error-state [error]="status.error()" [headingLevel]="3" (retry)="status.reload()" />
      } @else if (status.value(); as current) {
        @if (!current.enabled) {
          <p>{{ 'auth.mfa.intro' | transloco }}</p>
          <app-mfa-enrollment (finished)="dialog.close(true)" />
        } @else {
          <p class="message info" role="status">
            {{ 'auth.mfa.active' | transloco: { count: current.backupCodesLeft } }}
          </p>
          @if (current.requiredByPolicy) {
            <p>{{ 'auth.mfa.requiredByPolicy' | transloco }}</p>
          } @else {
            <form class="form" [formGroup]="form" (ngSubmit)="disable()" novalidate>
              <p>{{ 'auth.mfa.disableIntro' | transloco }}</p>
              <mat-form-field class="field" appearance="outline">
                <mat-label>{{ 'auth.mfa.codeOrBackup' | transloco }}</mat-label>
                <input matInput formControlName="code" autocomplete="one-time-code" required />
                <mat-error>{{ 'errors.field.required' | transloco }}</mat-error>
              </mat-form-field>
              @if (errorKey(); as key) {
                <p class="message error" role="alert">{{ key | transloco }}</p>
              }
              <button matButton="outlined" type="submit" [disabled]="submitting()">
                {{ 'auth.mfa.disable' | transloco }}
              </button>
            </form>
          }
        }
      }
    </mat-dialog-content>
    <mat-dialog-actions align="end">
      <button matButton type="button" mat-dialog-close>{{ 'auth.mfa.close' | transloco }}</button>
    </mat-dialog-actions>
  `,
})
export class MfaDialog {
  protected readonly dialog = inject<MatDialogRef<MfaDialog, boolean>>(MatDialogRef);
  private readonly api = inject(Api);

  protected readonly status = resource({ loader: () => this.api.invoke(getMfa) });
  protected readonly form = inject(FormBuilder).nonNullable.group({ code: ['', Validators.required] });
  protected readonly submitting = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  protected async disable(): Promise<void> {
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.submitting.set(true);
    this.errorKey.set(null);
    try {
      await this.api.invoke(disableMfa, { body: { code: this.form.getRawValue().code } });
      this.form.reset();
      this.status.reload();
    } catch (error) {
      this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
    } finally {
      this.submitting.set(false);
    }
  }
}
