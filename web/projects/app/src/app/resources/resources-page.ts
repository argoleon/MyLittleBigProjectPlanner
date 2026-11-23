import { Component, computed, effect, inject, resource, signal } from '@angular/core';
import { toSignal } from '@angular/core/rxjs-interop';
import { FormControl, ReactiveFormsModule } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatChipsModule } from '@angular/material/chips';
import { MatDialog } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatSelectModule } from '@angular/material/select';
import { MatTableModule } from '@angular/material/table';
import { RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import { debounceTime } from 'rxjs';
import {
  Api,
  Avatar,
  EmptyState,
  ErrorState,
  Resource,
  ResourceKindChip,
  ResourceStateChip,
  errorTranslationKey,
  isApiProblem,
  listTeams,
  resourceHolding,
} from 'ui';

import { ResourceDialog, ResourceDialogData } from './resource-dialog';
import { ResourcesStore } from './resources-store';

/** Outcome of the last action, shown above the table. */
interface Notice {
  readonly ok: boolean;
  readonly messageKey: string;
  readonly params?: Record<string, string>;
}

/**
 * Resources of the tenant (specification §8, L07-US01), under Workload:
 * search, filters by kind, status and team, the table of persons and generic
 * roles, each opening its page. Every internal member reads it; tenant
 * admins and resource managers create and change resources.
 */
@Component({
  selector: 'app-resources-page',
  imports: [
    Avatar,
    EmptyState,
    ErrorState,
    MatButtonModule,
    MatChipsModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    MatProgressBarModule,
    MatSelectModule,
    MatTableModule,
    ReactiveFormsModule,
    ResourceKindChip,
    ResourceStateChip,
    RouterLink,
    TranslocoPipe,
  ],
  providers: [ResourcesStore],
  templateUrl: './resources-page.html',
  styleUrl: './resources-page.scss',
})
export class ResourcesPage {
  protected readonly store = inject(ResourcesStore);
  private readonly dialog = inject(MatDialog);
  private readonly api = inject(Api);

  /** The teams to filter by; none when they cannot be read. */
  private readonly teamList = resource({ loader: () => this.api.invoke(listTeams) });
  protected readonly teams = computed(() => (this.teamList.hasValue() ? this.teamList.value().items : []));

  protected readonly columns = ['name', 'holding', 'status', 'actions'];
  protected readonly searchControl = new FormControl('', { nonNullable: true });
  private readonly search = toSignal(this.searchControl.valueChanges.pipe(debounceTime(300)), { initialValue: '' });

  protected readonly notice = signal<Notice | null>(null);

  protected readonly holding = resourceHolding;
  protected readonly trackResource = (_: number, resource: Resource) => resource.id;

  constructor() {
    effect(() => this.store.search.set(this.search()));
    effect(() => {
      const error = this.store.moreError();
      if (error) this.notice.set({ ok: false, messageKey: messageOf(error) });
    });
  }

  /** `id` of the edit button of `resource`, where the focus goes back. */
  protected editId(resource: Resource): string {
    return `resource-edit-${resource.id}`;
  }

  protected create(): void {
    this.open({}, undefined, 'resources.created');
  }

  protected edit(resource: Resource): void {
    this.open({ resource }, `#${this.editId(resource)}`, 'resources.saved');
  }

  private open(data: ResourceDialogData, restoreFocus: string | undefined, messageKey: string): void {
    this.dialog
      .open<ResourceDialog, ResourceDialogData, Resource>(ResourceDialog, {
        data,
        width: 'min(560px, 100vw)',
        autoFocus: 'first-tabbable',
        restoreFocus: restoreFocus ?? true,
      })
      .afterClosed()
      .subscribe((saved) => {
        if (!saved) return;
        this.notice.set({ ok: true, messageKey, params: { name: saved.displayName } });
        this.store.reload();
      });
  }
}

function messageOf(error: unknown): string {
  return isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown');
}
