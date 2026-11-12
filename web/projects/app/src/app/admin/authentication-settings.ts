import { Component, computed, inject, resource, signal } from '@angular/core';
import { FormBuilder, ReactiveFormsModule } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatSlideToggleModule } from '@angular/material/slide-toggle';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  AuthPolicy,
  ErrorState,
  OidcProvider,
  deleteOidcProvider,
  errorTranslationKey,
  getAuthPolicy,
  isApiProblem,
  listOidcProviders,
  testOidcProvider,
  updateAuthPolicy,
} from 'ui';

import { ProviderDialog, ProviderDialogData } from './provider-dialog';

/** The policy's toggles, in display order. */
export const POLICY_TOGGLES: readonly (keyof AuthPolicy)[] = [
  'internalAccountsRequireSso',
  'localAccountsRequireMfa',
  'portalPasswordEnabled',
  'portalMagicLinkEnabled',
];

/** Outcome of a provider's connection test, by provider id. */
interface Check {
  readonly ok: boolean;
  readonly messageKey: string;
  readonly detail?: string;
}

/**
 * "Authentication" tab of the administration (mockup 09): the tenant's
 * policy, and its OIDC providers. Tenant admins only; others see the API's
 * refusal.
 */
@Component({
  selector: 'app-authentication-settings',
  imports: [
    ErrorState,
    MatButtonModule,
    MatIconModule,
    MatProgressBarModule,
    MatSlideToggleModule,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  styleUrl: './authentication-settings.scss',
  templateUrl: './authentication-settings.html',
})
export class AuthenticationSettings {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);

  protected readonly toggles = POLICY_TOGGLES;
  protected readonly policy = resource({
    loader: async () => {
      const response = await this.api.invoke$Response(getAuthPolicy);
      this.form.setValue(response.body);
      this.form.markAsPristine();
      return { body: response.body, etag: response.headers.get('ETag') ?? '*' };
    },
  });
  protected readonly providers = resource({ loader: () => this.api.invoke(listOidcProviders) });

  protected readonly form = inject(FormBuilder).nonNullable.group({
    internalAccountsRequireSso: [false],
    localAccountsRequireMfa: [false],
    portalPasswordEnabled: [true],
    portalMagicLinkEnabled: [true],
  });
  protected readonly saving = signal(false);
  protected readonly policyMessage = signal<{ ok: boolean; key: string } | null>(null);
  protected readonly checks = signal<Readonly<Record<string, Check>>>({});
  protected readonly providerError = signal<string | null>(null);

  /** SSO is required but no provider is enabled: the rule waits for one. */
  protected readonly ssoWithoutProvider = computed(() => {
    const providers = this.providers.hasValue() ? this.providers.value() : [];
    return this.ssoRequired() && !providers.some((p) => p.enabled);
  });
  private readonly ssoRequired = signal(false);

  constructor() {
    this.form.controls.internalAccountsRequireSso.valueChanges.subscribe((value) => this.ssoRequired.set(value));
  }

  protected async savePolicy(): Promise<void> {
    const current = this.policy.hasValue() ? this.policy.value() : null;
    if (!current) return;
    this.saving.set(true);
    this.policyMessage.set(null);
    try {
      const response = await this.api.invoke$Response(updateAuthPolicy, {
        'If-Match': current.etag,
        body: this.form.getRawValue(),
      });
      // The answer is the new version: no reload, the card stays in place.
      this.policy.set({ body: response.body, etag: response.headers.get('ETag') ?? '*' });
      this.form.markAsPristine();
      this.policyMessage.set({ ok: true, key: 'admin.auth.policySaved' });
    } catch (error) {
      this.policyMessage.set({ ok: false, key: keyOf(error) });
    } finally {
      this.saving.set(false);
    }
  }

  protected edit(provider?: OidcProvider): void {
    const data: ProviderDialogData = { provider };
    this.dialog
      .open<ProviderDialog, ProviderDialogData, OidcProvider>(ProviderDialog, {
        data,
        width: 'min(560px, 100vw)',
        autoFocus: 'first-tabbable',
      })
      .afterClosed()
      .subscribe((saved) => {
        if (saved) this.providers.reload();
      });
  }

  protected async test(provider: OidcProvider): Promise<void> {
    try {
      await this.api.invoke(testOidcProvider, { id: provider.id });
      this.setCheck(provider.id, { ok: true, messageKey: 'admin.auth.testOk' });
    } catch (error) {
      const detail = isApiProblem(error) ? (error.detail ?? undefined) : undefined;
      this.setCheck(provider.id, { ok: false, messageKey: keyOf(error), detail });
    }
  }

  protected async remove(provider: OidcProvider): Promise<void> {
    this.providerError.set(null);
    try {
      await this.api.invoke(deleteOidcProvider, { id: provider.id, 'If-Match': `"${provider.version}"` });
      this.providers.reload();
    } catch (error) {
      this.providerError.set(keyOf(error));
    }
  }

  private setCheck(id: string, check: Check): void {
    this.checks.update((checks) => ({ ...checks, [id]: check }));
  }
}

function keyOf(error: unknown): string {
  return isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown');
}
