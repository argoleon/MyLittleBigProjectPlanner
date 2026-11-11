import { Component, computed, inject, resource } from '@angular/core';
import { MatDialog } from '@angular/material/dialog';
import { MatSnackBar } from '@angular/material/snack-bar';
import { RouterOutlet } from '@angular/router';
import { TranslocoService } from '@jsverse/transloco';
import {
  AccountMenu,
  Api,
  AuthService,
  Shell,
  TenantOption,
  TenantSwitcher,
  errorTranslationKey,
  isApiProblem,
  listMyMemberships,
} from 'ui';

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

  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly snackBar = inject(MatSnackBar);
  private readonly transloco = inject(TranslocoService);
  private readonly memberships = resource({ loader: () => this.api.invoke(listMyMemberships) });

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
