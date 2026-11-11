import { Component, computed, input, output } from '@angular/core';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatSelectChange, MatSelectModule } from '@angular/material/select';
import { TranslocoPipe } from '@jsverse/transloco';

/** A tenant the user can switch to. */
export interface TenantOption {
  readonly id: string;
  readonly name: string;
  /** Translation key of a second line, such as the user's role there. */
  readonly detailKey?: string;
}

/**
 * Organization selector at the top of the drawer: the user's tenants, the
 * current one selected. Picking another emits `switched` and leaves the
 * current one shown: the page is left for the other tenant, or stays here
 * if the switch fails. Disabled while no tenant is known (loading, API
 * unreachable).
 */
@Component({
  selector: 'ui-tenant-switcher',
  imports: [MatFormFieldModule, MatSelectModule, TranslocoPipe],
  styleUrl: './tenant-switcher.scss',
  template: `
    <mat-form-field appearance="outline" subscriptSizing="dynamic" class="field">
      <mat-label>{{ 'shell.tenant' | transloco }}</mat-label>
      <mat-select [value]="currentId()" [disabled]="tenants().length === 0" (selectionChange)="pick($event)">
        <mat-select-trigger>{{ currentName() }}</mat-select-trigger>
        @for (tenant of tenants(); track tenant.id) {
          <mat-option [value]="tenant.id">
            <span class="name">{{ tenant.name }}</span>
            @if (tenant.detailKey; as key) {
              <span class="detail">{{ key | transloco }}</span>
            }
          </mat-option>
        }
      </mat-select>
    </mat-form-field>
  `,
})
export class TenantSwitcher {
  readonly tenants = input<readonly TenantOption[]>([]);
  /** Id of the current tenant; the first one by default. */
  readonly current = input<string | null>(null);
  /** Id of the tenant the user picked, other than the current one. */
  readonly switched = output<string>();

  protected readonly currentId = computed(() => this.current() ?? this.tenants()[0]?.id ?? null);
  protected readonly currentName = computed(
    () => this.tenants().find((tenant) => tenant.id === this.currentId())?.name ?? '',
  );

  protected pick(change: MatSelectChange<string>): void {
    const picked = change.value;
    // The current tenant stays selected until the page is left.
    change.source.value = this.currentId();
    if (picked !== this.currentId()) {
      this.switched.emit(picked);
    }
  }
}
