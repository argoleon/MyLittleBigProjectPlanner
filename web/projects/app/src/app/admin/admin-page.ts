import { Component, computed, inject, resource } from '@angular/core';
import { MatTabsModule } from '@angular/material/tabs';
import { RouterLink, RouterLinkActive, RouterOutlet } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, getCurrentTenant } from 'ui';

/** A tab of the administration screen. */
export interface AdminTab {
  readonly path: string;
  readonly labelKey: string;
}

/**
 * Tabs of the administration (mockup 09). Members, groups, roles, project
 * configurations and quotas come with their work packages.
 */
export const ADMIN_TABS: readonly AdminTab[] = [{ path: 'authentication', labelKey: 'admin.tabs.authentication' }];

/** Administration screen of the tenant: routed tabs. */
@Component({
  selector: 'app-admin-page',
  imports: [MatTabsModule, RouterLink, RouterLinkActive, RouterOutlet, TranslocoPipe],
  styleUrl: './admin-page.scss',
  template: `
    <p class="eyebrow">
      {{ 'nav.administration' | transloco }}
      @if (tenantName(); as name) {
        · {{ name }}
      }
    </p>
    <h1 class="title">{{ 'nav.administration' | transloco }}</h1>
    <nav mat-tab-nav-bar [tabPanel]="panel" [attr.aria-label]="'admin.tabs.label' | transloco">
      @for (tab of tabs; track tab.path) {
        <a mat-tab-link [routerLink]="tab.path" routerLinkActive #active="routerLinkActive" [active]="active.isActive">
          {{ tab.labelKey | transloco }}
        </a>
      }
    </nav>
    <mat-tab-nav-panel #panel class="panel">
      <router-outlet />
    </mat-tab-nav-panel>
  `,
})
export class AdminPage {
  protected readonly tabs = ADMIN_TABS;
  private readonly api = inject(Api);
  private readonly tenant = resource({ loader: () => this.api.invoke(getCurrentTenant) });
  protected readonly tenantName = computed(() => (this.tenant.hasValue() ? this.tenant.value().name : null));
}
