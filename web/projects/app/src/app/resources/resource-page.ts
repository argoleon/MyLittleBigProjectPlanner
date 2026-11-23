import { Component, computed, inject, input, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  AuthService,
  Avatar,
  ErrorState,
  LoadingState,
  Resource,
  ResourceKindChip,
  ResourceStateChip,
  TenantRole,
  getResource,
  resourceHolding,
} from 'ui';

import { CapacityCard } from '../capacity/capacity-card';
import { ResourceDialog, ResourceDialogData } from './resource-dialog';

/** Whether a member with `role` creates and changes resources. */
export function managesResources(role: TenantRole | undefined): boolean {
  return role === 'tenant_admin' || role === 'resource_manager';
}

/**
 * The page of a resource (mockup 20, L07-US01): its name, kind, job title,
 * account, status and weekly capacity (L07-US03). Its calendar and absences
 * come with the next stories of L07.
 */
@Component({
  selector: 'app-resource-page',
  imports: [Avatar, CapacityCard, ErrorState, LoadingState, MatButtonModule, MatIconModule, ResourceKindChip, ResourceStateChip, RouterLink, TranslocoPipe],
  template: `
    <nav class="breadcrumb" [attr.aria-label]="'resources.breadcrumb' | transloco">
      <span>{{ 'nav.workload' | transloco }}</span>
      <mat-icon aria-hidden="true">chevron_right</mat-icon>
      <a routerLink="/workload/resources">{{ 'resources.title' | transloco }}</a>
    </nav>
    @if (current.error()) {
      <ui-error-state [error]="current.error()" (retry)="current.reload()" />
    } @else if (shown(); as shown) {
      <div class="header">
        <ui-avatar [name]="shown.displayName" size="medium" aria-hidden="true" />
        <div class="identity">
          <h1 class="title">{{ shown.displayName }}</h1>
          <div class="chips">
            <ui-resource-kind-chip [holding]="holding(shown)" />
            @if (shown.jobTitle) {
              <span class="job-title">{{ shown.jobTitle }}</span>
            }
            <ui-resource-state-chip [active]="shown.active" />
          </div>
        </div>
        @if (canManage()) {
          <button matButton="outlined" type="button" class="edit" id="resource-edit" (click)="edit(shown)">
            <mat-icon aria-hidden="true">edit</mat-icon>
            {{ 'resources.page.edit' | transloco }}
          </button>
        }
      </div>
      <section class="card" aria-labelledby="resource-details">
        <h2 class="card-title" id="resource-details">{{ 'resources.page.details' | transloco }}</h2>
        <dl class="details">
          <dt>{{ 'resources.page.kind' | transloco }}</dt>
          <dd>{{ 'resource.holding.' + holding(shown) | transloco }}</dd>
          <dt>{{ 'resources.page.account' | transloco }}</dt>
          <dd>{{ 'resources.page.' + holding(shown) | transloco }}</dd>
          <dt>{{ 'resources.page.teams' | transloco }}</dt>
          <dd class="teams">
            @for (team of shown.teams; track team.id; let last = $last) {
              <a [routerLink]="['/workload/teams', team.id]">{{ team.name }}</a>{{ last ? '' : ', ' }}
            } @empty {
              {{ 'resources.page.noTeam' | transloco }}
            }
          </dd>
          <dt>{{ 'resources.page.jobTitle' | transloco }}</dt>
          <dd>{{ shown.jobTitle ?? '–' }}</dd>
          <dt>{{ 'resources.page.status' | transloco }}</dt>
          <dd>{{ (shown.active ? 'resource.state.active' : 'resource.state.inactive') | transloco }}</dd>
        </dl>
        <p class="next">{{ 'resources.page.next' | transloco }}</p>
      </section>
      <app-capacity-card class="capacity" [resourceId]="shown.id" />
    } @else {
      <ui-loading-state [label]="'resources.loading' | transloco" />
    }
  `,
  styleUrl: './resource-page.scss',
})
export class ResourcePage {
  /** `:id` of the route. */
  readonly id = input.required<string>();

  private readonly api = inject(Api);
  private readonly auth = inject(AuthService);
  private readonly dialog = inject(MatDialog);

  protected readonly current = resource({
    params: () => this.id(),
    loader: ({ params }) => this.api.invoke(getResource, { id: params }),
  });
  /** Saved by the dialog since the page was loaded. */
  private readonly saved = signal<Resource | null>(null);
  protected readonly shown = computed<Resource | null>(() => {
    const saved = this.saved();
    if (saved?.id === this.id()) return saved;
    return this.current.hasValue() ? this.current.value() : null;
  });
  protected readonly canManage = computed(() => managesResources(this.auth.session()?.account.tenantRole));
  protected readonly holding = resourceHolding;

  protected edit(resource: Resource): void {
    this.dialog
      .open<ResourceDialog, ResourceDialogData, Resource>(ResourceDialog, {
        data: { resource },
        width: 'min(560px, 100vw)',
        autoFocus: 'first-tabbable',
        restoreFocus: '#resource-edit',
      })
      .afterClosed()
      .subscribe((saved) => {
        if (saved) this.saved.set(saved);
      });
  }
}
