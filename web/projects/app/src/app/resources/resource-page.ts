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

import { AbsencesCard } from '../absences/absences-card';
import { MonthCalendar } from '../calendars/month-calendar';
import { CapacityCard } from '../capacity/capacity-card';
import { ResourceDialog, ResourceDialogData } from './resource-dialog';

/** Whether a member with `role` creates and changes resources. */
export function managesResources(role: TenantRole | undefined): boolean {
  return role === 'tenant_admin' || role === 'resource_manager';
}

/**
 * The page of a resource (mockup 20, L07-US01): its name, kind, job title,
 * account, status, site, weekly capacity (L07-US03) and month calendar
 * (L07-US04), and its absences (L07-US05), which the person and the
 * managers declare.
 */
@Component({
  selector: 'app-resource-page',
  imports: [
    AbsencesCard,
    Avatar,
    CapacityCard,
    ErrorState,
    LoadingState,
    MatButtonModule,
    MatIconModule,
    MonthCalendar,
    ResourceKindChip,
    ResourceStateChip,
    RouterLink,
    TranslocoPipe,
  ],
  templateUrl: './resource-page.html',
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
