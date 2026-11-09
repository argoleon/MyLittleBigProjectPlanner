import { Component, computed, inject, resource } from '@angular/core';
import { RouterOutlet } from '@angular/router';
import { Api, Shell, TenantOption, TenantSwitcher, getCurrentTenant } from 'ui';

import { NAVIGATION } from './navigation';

@Component({
  selector: 'app-root',
  imports: [RouterOutlet, Shell, TenantSwitcher],
  template: `
    <ui-shell [navItems]="navigation">
      <ui-tenant-switcher uiShellDrawerHeader [tenants]="tenants()" />
      <router-outlet />
    </ui-shell>
  `,
})
export class App {
  protected readonly navigation = NAVIGATION;

  private readonly api = inject(Api);
  private readonly tenant = resource({ loader: () => this.api.invoke(getCurrentTenant) });

  /** The current tenant; empty while loading or if the API is unreachable. */
  protected readonly tenants = computed<TenantOption[]>(() => {
    const tenant = this.tenant.hasValue() ? this.tenant.value() : undefined;
    return tenant ? [{ id: tenant.id, name: tenant.name }] : [];
  });
}
