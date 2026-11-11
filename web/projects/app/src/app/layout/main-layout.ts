import { Component, computed, inject, resource } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { MatDialog } from '@angular/material/dialog';
import { MatSnackBar } from '@angular/material/snack-bar';
import { Router, RouterOutlet } from '@angular/router';
import { TranslocoService } from '@jsverse/transloco';
import {
  AccountMenu,
  Api,
  AuthService,
  Language,
  LocaleService,
  Shell,
  TenantOption,
  TenantSwitcher,
  errorTranslationKey,
  isApiProblem,
  listMyMemberships,
} from 'ui';

import { MyAccount } from '../account/my-account';
import { NAVIGATION } from '../navigation';
import { MfaDialog } from './mfa-dialog';

/** Frame of the screens of signed-in members: navigation, top app bar, account. */
@Component({
  selector: 'app-main-layout',
  imports: [AccountMenu, RouterOutlet, Shell, TenantSwitcher],
  template: `
    <ui-shell [navItems]="navigation">
      <ui-tenant-switcher
        uiShellDrawerHeader
        [tenants]="tenants()"
        [current]="currentTenant()"
        (switched)="switchTo($event)"
      />
      @if (auth.session(); as session) {
        <ui-account-menu
          uiShellTopBar
          [displayName]="session.account.displayName"
          [email]="session.account.email"
          (signOut)="auth.signOut()"
          (account)="router.navigate(['/account'])"
          (security)="openSecurity()"
        />
      }
      <router-outlet />
    </ui-shell>
  `,
})
export class MainLayout {
  protected readonly navigation = NAVIGATION;
  protected readonly auth = inject(AuthService);
  protected readonly router = inject(Router);
  private readonly myAccount = inject(MyAccount);

  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly snackBar = inject(MatSnackBar);
  private readonly transloco = inject(TranslocoService);
  private readonly memberships = resource({ loader: () => this.api.invoke(listMyMemberships) });

  constructor() {
    // The account's language and time zone; the device's until loaded, or
    // if the API cannot answer.
    this.myAccount.load().catch(() => undefined);
    inject(LocaleService)
      .picked.pipe(takeUntilDestroyed())
      .subscribe((language) => this.keepLanguage(language));
  }

  /** Saves the language picked in the menu for the account. */
  private async keepLanguage(language: Language): Promise<void> {
    const preferences = this.myAccount.preferences();
    if (!preferences || preferences.locale === language) return;
    try {
      await this.myAccount.save({ ...preferences, locale: language });
    } catch (error) {
      const key = isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown');
      this.snackBar.open(this.transloco.translate(key), undefined, { duration: 8000 });
    }
  }

  protected openSecurity(): void {
    this.dialog.open(MfaDialog, { width: 'min(560px, 100vw)', autoFocus: 'first-heading' });
  }

  /** The member's tenants, with their role in each; empty while loading or if the API is unreachable. */
  protected readonly tenants = computed<TenantOption[]>(() =>
    (this.memberships.hasValue() ? this.memberships.value() : []).map((membership) => ({
      id: membership.tenantId,
      name: membership.name,
      detailKey: `shell.role.${membership.tenantRole}`,
    })),
  );

  protected readonly currentTenant = computed(() =>
    this.memberships.hasValue() ? (this.memberships.value().find((m) => m.current)?.tenantId ?? null) : null,
  );

  /** Leaves for the tenant picked; stays here with a message if the API refuses. */
  protected async switchTo(tenantId: string): Promise<void> {
    try {
      await this.auth.switchTenant(tenantId);
    } catch (error) {
      const key = isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown');
      this.snackBar.open(this.transloco.translate(key), undefined, { duration: 8000 });
    }
  }
}
