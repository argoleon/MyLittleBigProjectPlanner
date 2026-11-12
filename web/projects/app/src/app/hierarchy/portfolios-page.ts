import { Component, computed, inject, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatChipsModule } from '@angular/material/chips';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatTableModule } from '@angular/material/table';
import { Router, RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  AuthService,
  EmptyState,
  ErrorState,
  HealthChip,
  LocalDatePipe,
  Node,
  NodeType,
  NodeTypeChip,
  listNodes,
} from 'ui';

import { NodeDialog, NodeDialogData } from './node-dialog';
import { NODE_TYPES } from './node-form';

/**
 * The portfolios, programs and projects the member sees, in creation
 * order, with a filter by type. Tenant admins create nodes at the root
 * here. (The portfolio view with its aggregates comes with L03-US05.)
 */
@Component({
  selector: 'app-portfolios-page',
  imports: [
    EmptyState,
    ErrorState,
    HealthChip,
    LocalDatePipe,
    MatButtonModule,
    MatChipsModule,
    MatIconModule,
    MatProgressBarModule,
    MatTableModule,
    NodeTypeChip,
    RouterLink,
    TranslocoPipe,
  ],
  styleUrl: './portfolios-page.scss',
  templateUrl: './portfolios-page.html',
})
export class PortfoliosPage {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly router = inject(Router);
  private readonly auth = inject(AuthService);

  protected readonly types = NODE_TYPES;
  protected readonly columns = ['name', 'parents', 'health', 'owners', 'dates'];
  /** Only tenant admins create at the root. */
  protected readonly canCreate = computed(() => this.auth.session()?.account.tenantRole === 'tenant_admin');
  protected readonly filter = signal<NodeType | 'all'>('all');
  protected readonly nodes = resource({
    // The hierarchy browse of L03-US05 pages and aggregates; every page here.
    loader: async () => {
      const all: Node[] = [];
      let cursor: string | undefined;
      do {
        const page = await this.api.invoke(listNodes, { limit: 200, cursor });
        all.push(...page.items);
        cursor = page.nextCursor ?? undefined;
      } while (cursor);
      return all;
    },
  });
  protected readonly shown = computed(() => {
    const all = this.nodes.hasValue() ? this.nodes.value() : [];
    const type = this.filter();
    return type === 'all' ? all : all.filter((node) => node.type === type);
  });

  protected ownersOf(node: Node): string {
    return node.owners.map((owner) => owner.displayName).join(', ');
  }

  protected parentsOf(node: Node): string {
    return node.parents.map((parent) => parent.name).join(', ');
  }

  protected create(): void {
    this.dialog
      .open<NodeDialog, NodeDialogData, Node>(NodeDialog, { data: {}, width: 'min(560px, 100vw)', autoFocus: 'first-tabbable' })
      .afterClosed()
      .subscribe((node) => {
        if (node) void this.router.navigate(['/nodes', node.id]);
      });
  }
}
