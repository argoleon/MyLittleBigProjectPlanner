import { inject } from '@angular/core';
import { CanActivateFn, Router } from '@angular/router';
import { AuthService, TenantRole } from 'ui';

/** A tab of the administration screen, and the tenant roles that see it. */
export interface AdminTab {
  readonly path: string;
  readonly labelKey: string;
  readonly roles: readonly TenantRole[];
}

/**
 * Tabs of the administration (mockup 09): configuration admins only manage
 * the project configurations (§3). Quotas come with their work package.
 */
export const ADMIN_TABS: readonly AdminTab[] = [
  { path: 'members', labelKey: 'admin.tabs.members', roles: ['tenant_admin'] },
  { path: 'groups', labelKey: 'admin.tabs.groups', roles: ['tenant_admin'] },
  { path: 'roles', labelKey: 'admin.tabs.roles', roles: ['tenant_admin'] },
  { path: 'authentication', labelKey: 'admin.tabs.authentication', roles: ['tenant_admin'] },
  { path: 'configurations', labelKey: 'admin.tabs.configurations', roles: ['tenant_admin', 'configuration_admin'] },
];

/**
 * `/admin` opens on the first tab of the member's role: Members for tenant
 * admins, Project configurations for configuration admins. A guard, not a
 * redirect, so that it runs once the session is loaded.
 */
export const adminHomeGuard: CanActivateFn = () => {
  const role = inject(AuthService).session()?.account.tenantRole;
  const first = ADMIN_TABS.find((tab) => role !== undefined && tab.roles.includes(role)) ?? ADMIN_TABS[0];
  return inject(Router).createUrlTree(['/admin', first.path]);
};
