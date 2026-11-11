import { Component, inject, signal } from '@angular/core';
import { FormBuilder, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { MatSlideToggleModule } from '@angular/material/slide-toggle';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  OidcProvider,
  OidcProviderInput,
  createOidcProvider,
  errorTranslationKey,
  fieldErrorTranslationKey,
  isApiProblem,
  updateOidcProvider,
} from 'ui';

/** What the dialog edits: nothing for a new provider. */
export interface ProviderDialogData {
  readonly provider?: OidcProvider;
}

/** The form's text fields, as named by the API. */
type Field = 'displayName' | 'issuer' | 'clientId' | 'clientSecret' | 'emailClaim';

/**
 * Adds or changes an OIDC provider. The client secret is write-only: on
 * change, leaving it empty keeps the stored one. Closes with the saved
 * provider.
 */
@Component({
  selector: 'app-provider-dialog',
  imports: [
    MatButtonModule,
    MatDialogModule,
    MatFormFieldModule,
    MatInputModule,
    MatSlideToggleModule,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  styleUrl: './provider-dialog.scss',
  templateUrl: './provider-dialog.html',
})
export class ProviderDialog {
  protected readonly data = inject<ProviderDialogData>(MAT_DIALOG_DATA);
  private readonly dialog = inject<MatDialogRef<ProviderDialog, OidcProvider>>(MatDialogRef);
  private readonly api = inject(Api);

  protected readonly editing = !!this.data.provider;
  protected readonly form = inject(FormBuilder).nonNullable.group({
    displayName: [this.data.provider?.displayName ?? '', [Validators.required, Validators.maxLength(100)]],
    issuer: [this.data.provider?.issuer ?? '', [Validators.required, Validators.pattern(/^https?:\/\/\S+$/)]],
    clientId: [this.data.provider?.clientId ?? '', [Validators.required, Validators.maxLength(500)]],
    clientSecret: ['', this.editing ? [] : [Validators.required]],
    emailClaim: [
      this.data.provider?.emailClaim ?? 'email',
      [Validators.required, Validators.pattern(/^[A-Za-z0-9_:.-]{1,100}$/)],
    ],
    enabled: [this.data.provider?.enabled ?? true],
  });
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  /** Translation key of the first error of `field`. */
  protected fieldError(field: Field): string | null {
    const control = this.form.controls[field];
    if (control.hasError('required')) return 'errors.field.required';
    if (control.hasError('pattern')) return 'errors.field.format';
    if (control.hasError('maxlength')) return 'errors.field.too_long';
    return (control.getError('api') as string | undefined) ?? null;
  }

  protected async save(): Promise<void> {
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    const value = this.form.getRawValue();
    const body: OidcProviderInput = {
      displayName: value.displayName.trim(),
      issuer: value.issuer.trim(),
      clientId: value.clientId.trim(),
      emailClaim: value.emailClaim.trim(),
      enabled: value.enabled,
      ...(value.clientSecret ? { clientSecret: value.clientSecret } : {}),
    };
    try {
      const provider = this.data.provider;
      const saved = provider
        ? await this.api.invoke(updateOidcProvider, { id: provider.id, 'If-Match': `"${provider.version}"`, body })
        : await this.api.invoke(createOidcProvider, { body });
      this.dialog.close(saved);
    } catch (error) {
      if (isApiProblem(error) && error.fieldErrors.length > 0) {
        for (const fieldError of error.fieldErrors) {
          const control = this.form.get(fieldError.field);
          control?.setErrors({ api: fieldErrorTranslationKey(fieldError.code) });
          control?.markAsTouched();
        }
      }
      this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
    } finally {
      this.saving.set(false);
    }
  }
}
