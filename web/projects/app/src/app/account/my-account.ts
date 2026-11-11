import { Injectable, computed, inject, signal } from '@angular/core';
import {
  AccountPreferences,
  Api,
  LocaleService,
  Me,
  Tenant,
  getCurrentTenant,
  getMe,
  isLanguage,
  updateMyPreferences,
} from 'ui';

/**
 * The account's language and time zone (L02-US08), applied to the
 * interface. They follow the account in every organization and on every
 * device. Without a choice of the account, the language stays the one of
 * this device (picked before, or the browser's) and instants are shown in
 * the organization's time zone.
 */
@Injectable({ providedIn: 'root' })
export class MyAccount {
  private readonly api = inject(Api);
  private readonly locale = inject(LocaleService);

  private readonly meSignal = signal<Me | null>(null);
  private readonly tenantSignal = signal<Tenant | null>(null);

  /** The signed-in account, once loaded. */
  readonly me = this.meSignal.asReadonly();
  /** The current organization, whose defaults apply when the account has none. */
  readonly tenant = this.tenantSignal.asReadonly();
  /** The account's own choices; `null` parts follow the organization. */
  readonly preferences = computed<AccountPreferences | null>(() => {
    const me = this.meSignal();
    return me ? { locale: me.locale ?? null, timeZone: me.timeZone ?? null } : null;
  });

  private loading: Promise<void> | null = null;

  /**
   * Loads the account and the organization, then applies the preferences.
   * Once: later calls wait for the same load, or retry after a failure.
   */
  load(): Promise<void> {
    this.loading ??= Promise.all([this.api.invoke(getMe), this.api.invoke(getCurrentTenant)]).then(
      ([me, tenant]) => {
        this.meSignal.set(me);
        this.tenantSignal.set(tenant);
        this.apply();
      },
      (error: unknown) => {
        this.loading = null;
        throw error;
      },
    );
    return this.loading;
  }

  /** Saves `preferences` for the account, then applies them. */
  async save(preferences: AccountPreferences): Promise<void> {
    const saved = await this.api.invoke(updateMyPreferences, { body: preferences });
    this.meSignal.update((me) => (me ? { ...me, locale: saved.locale ?? null, timeZone: saved.timeZone ?? null } : me));
    this.apply();
  }

  /** Sets the interface's language and time zone from what is known. */
  apply(): void {
    const me = this.meSignal();
    const tenant = this.tenantSignal();
    const language = me?.locale;
    if (isLanguage(language) && language !== this.locale.language()) {
      this.locale.use(language);
    }
    this.locale.useTimeZone(me?.timeZone ?? tenant?.timeZone ?? null);
  }
}
