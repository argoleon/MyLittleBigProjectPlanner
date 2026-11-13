import { Component, computed, inject, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatButtonToggleModule } from '@angular/material/button-toggle';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatTableModule } from '@angular/material/table';
import { Router, RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, AuthService, EmptyState, ErrorState, Node, NodeItem, listNodes } from 'ui';

import { HierarchyTree } from './hierarchy-tree';
import { NodeDialog, NodeDialogData } from './node-dialog';
import { VeiledName, isVeiled } from './veiled';

/** What the page lists: the hierarchy of live work, or the archived projects. */
export type PortfoliosView = 'active' | 'archived';

/**
 * The top of the hierarchy as the member sees it: the portfolios, programs
 * and projects without a parent, or whose parents they do not see, each
 * with its aggregated indicators. A row expands to what is under it; a
 * confidential project the member has no access to shows veiled there.
 * Tenant admins create nodes at the root here. The "Archived" view lists
 * the archived projects the member sees, to open or restore them.
 */
@Component({
  selector: 'app-portfolios-page',
  imports: [
    EmptyState,
    ErrorState,
    HierarchyTree,
    MatButtonModule,
    MatButtonToggleModule,
    MatIconModule,
    MatProgressBarModule,
    MatTableModule,
    RouterLink,
    TranslocoPipe,
    VeiledName,
  ],
  styleUrl: './portfolios-page.scss',
  templateUrl: './portfolios-page.html',
})
export class PortfoliosPage {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly router = inject(Router);
  private readonly auth = inject(AuthService);

  protected readonly columns = ['name', 'parents', 'owners'];
  /** Only tenant admins create at the root. */
  protected readonly canCreate = computed(() => this.auth.session()?.account.tenantRole === 'tenant_admin');
  protected readonly view = signal<PortfoliosView>('active');
  /** The archived projects, read when that view is chosen (every page). */
  protected readonly archived = resource({
    params: () => (this.view() === 'archived' ? true : undefined),
    loader: async () => {
      const all: NodeItem[] = [];
      let cursor: string | undefined;
      do {
        const page = await this.api.invoke(listNodes, { archived: true, limit: 200, cursor });
        all.push(...page.items);
        cursor = page.nextCursor ?? undefined;
      } while (cursor);
      return all;
    },
  });

  /** The node of a row, or null when it shows veiled (rows are untyped in the table's template). */
  protected asNode(item: NodeItem): Node | null {
    return isVeiled(item) ? null : item;
  }

  protected parentsOf(item: NodeItem): string {
    return item.parents.map((parent) => parent.name).join(', ');
  }

  protected ownersOf(item: NodeItem): string {
    return isVeiled(item) ? '' : item.owners.map((owner) => owner.displayName).join(', ');
  }

  protected create(): void {
    this.dialog
      .open<NodeDialog, NodeDialogData, Node>(NodeDialog, { data: {}, width: 'min(560px, 100vw)', autoFocus: 'first-tabbable' })
      .afterClosed()
      .subscribe((node) => {
        // On to its settings, to complete it.
        if (node) void this.router.navigate(['/nodes', node.id, 'general']);
      });
  }
}
