import { Component, computed, inject, resource } from '@angular/core';
import { MatDialog } from '@angular/material/dialog';
import { RouterOutlet } from '@angular/router';
import { AccountMenu, Api, AuthService, Shell, TenantOption, TenantSwitcher, getCurrentTenant } from 'ui';

import { NAVIGATION } from '../navigation';
import { MfaDialog } from './mfa-dialog';

/** Frame of the screens of signed-in members: navigation, top app bar, account. */
@Component({
  selector: 'app-main-layout',
  imports: [AccountMenu, RouterOutlet, Shell, TenantSwitcher],
  template: `
    <ui-shell [navItems]="navigation">
      <ui-tenant-switcher uiShellDrawerHeader [tenants]="tenants()" />
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
  private readonly tenant = resource({ loader: () => this.api.invoke(getCurrentTenant) });

  protected openSecurity(): void {
    this.dialog.open(MfaDialog, { width: 'min(560px, 100vw)', autoFocus: 'first-heading' });
  }

  /** The current tenant; empty while loading or if the API is unreachable. */
  protected readonly tenants = computed<TenantOption[]>(() => {
    const tenant = this.tenant.hasValue() ? this.tenant.value() : undefined;
    return tenant ? [{ id: tenant.id, name: tenant.name }] : [];
  });
}
