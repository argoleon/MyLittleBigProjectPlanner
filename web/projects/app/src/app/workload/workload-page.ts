import { Component } from '@angular/core';
import { MatTabsModule } from '@angular/material/tabs';
import { RouterLink, RouterLinkActive, RouterOutlet } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';

/** A tab of the Workload screen. */
interface WorkloadTab {
  readonly path: string;
  readonly labelKey: string;
}

/** The tabs, in order. The heatmap comes first once built (L08). */
export const WORKLOAD_TABS: readonly WorkloadTab[] = [
  { path: 'resources', labelKey: 'resources.title' },
  { path: 'teams', labelKey: 'teams.title' },
  { path: 'calendars', labelKey: 'calendars.title' },
];

/** The Workload screen (specification §8): routed tabs. */
@Component({
  selector: 'app-workload-page',
  imports: [MatTabsModule, RouterLink, RouterLinkActive, RouterOutlet, TranslocoPipe],
  template: `
    <h1 class="title">{{ 'nav.workload' | transloco }}</h1>
    <nav mat-tab-nav-bar [tabPanel]="panel" [attr.aria-label]="'workload.tabs' | transloco">
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
  styleUrl: './workload-page.scss',
})
export class WorkloadPage {
  protected readonly tabs = WORKLOAD_TABS;
}
