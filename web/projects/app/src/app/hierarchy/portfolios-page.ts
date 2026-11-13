import { Component, computed, inject } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { Router } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import { AuthService, EmptyState, Node } from 'ui';

import { HierarchyTree } from './hierarchy-tree';
import { NodeDialog, NodeDialogData } from './node-dialog';

/**
 * The top of the hierarchy as the member sees it: the portfolios, programs
 * and projects without a parent, or whose parents they do not see, each
 * with its aggregated indicators. A row expands to what is under it; a
 * confidential project the member has no access to shows veiled there.
 * Tenant admins create nodes at the root here.
 */
@Component({
  selector: 'app-portfolios-page',
  imports: [EmptyState, HierarchyTree, MatButtonModule, MatIconModule, TranslocoPipe],
  styleUrl: './portfolios-page.scss',
  templateUrl: './portfolios-page.html',
})
export class PortfoliosPage {
  private readonly dialog = inject(MatDialog);
  private readonly router = inject(Router);
  private readonly auth = inject(AuthService);

  /** Only tenant admins create at the root. */
  protected readonly canCreate = computed(() => this.auth.session()?.account.tenantRole === 'tenant_admin');

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
