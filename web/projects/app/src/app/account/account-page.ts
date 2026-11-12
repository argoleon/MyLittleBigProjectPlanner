import { Component, computed, inject, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatSelectModule } from '@angular/material/select';
import { MatSlideToggle, MatSlideToggleModule } from '@angular/material/slide-toggle';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  AccountPreferences,
  Api,
  ErrorState,
  InstantPipe,
  LANGUAGES,
  LANGUAGE_NAMES,
  errorTranslationKey,
  getMfa,
  isApiProblem,
  knownTimeZones,
  listMyMemberships,
  listMySessions,
} from 'ui';

import { MfaDialog } from '../layout/mfa-dialog';
import { MyAccount } from './my-account';
import { PasswordDialog } from './password-dialog';
import { PersonalDataCard } from './personal-data-card';
import { SessionsDialog } from './sessions-dialog';

/** What the language select holds: the account's language, or `auto`. */
const AUTO = 'auto';

/**
 * My account (mockup 10, L02-US08, L02-US09): preferences, security
 * (two-step verification, password, sessions), personal data (export,
 * erasure) and the account's organizations. API
 * tokens and authorized applications come with L15.
 */
@Component({
  selector: 'app-account-page',
  imports: [
    ErrorState,
    InstantPipe,
    MatButtonModule,
    MatFormFieldModule,
    MatIconModule,
    MatProgressBarModule,
    MatSelectModule,
    MatSlideToggleModule,
    PersonalDataCard,
    TranslocoPipe,
  ],
  styleUrl: './account-page.scss',
  templateUrl: './account-page.html',
})
export class AccountPage {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  protected readonly account = inject(MyAccount);

  protected readonly languages = LANGUAGES;
  protected readonly languageNames = LANGUAGE_NAMES;
  protected readonly auto = AUTO;
  protected readonly timeZones = knownTimeZones();

  protected readonly mfa = resource({ loader: () => this.api.invoke(getMfa) });
  protected readonly sessions = resource({ loader: () => this.api.invoke(listMySessions) });
  protected readonly memberships = resource({ loader: () => this.api.invoke(listMyMemberships) });

  /** The latest sign-in among the open sessions. */
  protected readonly lastSignIn = computed(() => {
    const sessions = this.sessions.hasValue() ? this.sessions.value() : [];
    return sessions.map((s) => s.createdAt).sort().at(-1) ?? null;
  });

  protected readonly languageValue = computed(() => this.account.preferences()?.locale ?? AUTO);
  protected readonly timeZoneValue = computed(() => this.account.preferences()?.timeZone ?? AUTO);
  protected readonly saving = signal(false);
  /** The account could not be loaded: the preferences cannot be shown. */
  protected readonly loadError = signal<unknown>(null);
  protected readonly notice = signal<{ ok: boolean; key: string } | null>(null);

  constructor() {
    this.loadAccount();
  }

  protected loadAccount(): void {
    this.loadError.set(null);
    this.account.load().catch((error: unknown) => this.loadError.set(error));
  }

  protected async saveLanguage(value: string): Promise<void> {
    const current = this.account.preferences();
    if (!current) return;
    await this.save({ ...current, locale: value === AUTO ? null : value });
  }

  protected async saveTimeZone(value: string): Promise<void> {
    const current = this.account.preferences();
    if (!current) return;
    await this.save({ ...current, timeZone: value === AUTO ? null : value });
  }

  private async save(preferences: AccountPreferences): Promise<void> {
    this.saving.set(true);
    try {
      await this.account.save(preferences);
      this.notice.set({ ok: true, key: 'account.preferences.saved' });
    } catch (error) {
      this.notice.set({ ok: false, key: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
    } finally {
      this.saving.set(false);
    }
  }

  /** The switch opens the dialog that enables or disables, then shows what holds. */
  protected openMfa(toggle: MatSlideToggle): void {
    toggle.checked = this.mfa.value()?.enabled ?? false;
    this.dialog
      .open(MfaDialog, { width: 'min(560px, 100vw)', autoFocus: 'first-heading' })
      .afterClosed()
      .subscribe(() => this.mfa.reload());
  }

  protected changePassword(): void {
    this.dialog
      .open<PasswordDialog, void, boolean>(PasswordDialog, { width: 'min(480px, 100vw)', autoFocus: 'first-tabbable' })
      .afterClosed()
      .subscribe((changed) => {
        if (!changed) return;
        this.notice.set({ ok: true, key: 'account.password.changed' });
        this.sessions.reload();
      });
  }

  protected manageSessions(): void {
    this.dialog
      .open(SessionsDialog, { width: 'min(640px, 100vw)', autoFocus: 'first-heading' })
      .afterClosed()
      .subscribe(() => this.sessions.reload());
  }
}
