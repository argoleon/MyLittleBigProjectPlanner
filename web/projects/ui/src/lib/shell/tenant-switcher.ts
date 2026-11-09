import { Component, computed, input, output } from '@angular/core';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatSelectModule } from '@angular/material/select';
import { TranslocoPipe } from '@jsverse/transloco';

/** A tenant the user can switch to. */
export interface TenantOption {
  readonly id: string;
  readonly name: string;
}

/**
 * Organization selector at the top of the drawer. Placeholder until users
 * can belong to several tenants (L02): today it shows the current one.
 * Disabled while no tenant is known (loading, API unreachable).
 */
@Component({
  selector: 'ui-tenant-switcher',
  imports: [MatFormFieldModule, MatSelectModule, TranslocoPipe],
  styleUrl: './tenant-switcher.scss',
  template: `
    <mat-form-field appearance="outline" subscriptSizing="dynamic" class="field">
      <mat-label>{{ 'shell.tenant' | transloco }}</mat-label>
      <mat-select [value]="currentId()" [disabled]="tenants().length === 0" (selectionChange)="switched.emit($event.value)">
        @for (tenant of tenants(); track tenant.id) {
          <mat-option [value]="tenant.id">{{ tenant.name }}</mat-option>
        }
      </mat-select>
    </mat-form-field>
  `,
})
export class TenantSwitcher {
  readonly tenants = input<readonly TenantOption[]>([]);
  /** Id of the current tenant; the first one by default. */
  readonly current = input<string | null>(null);
  /** Id of the tenant the user picked. */
  readonly switched = output<string>();

  protected readonly currentId = computed(() => this.current() ?? this.tenants()[0]?.id ?? null);
}
