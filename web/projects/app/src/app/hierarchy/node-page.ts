import { Component, computed, inject, input, resource, viewChild } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  AuthService,
  EmptyState,
  ErrorState,
  LocalDatePipe,
  LocalNumberPipe,
  Node,
  getNode,
  getNodeAggregate,
  listMyNodePermissions,
} from 'ui';

import { ArchivedBanner } from './archived-banner';
import { PERCENT, hoursOf } from './hierarchy-rows';
import { HierarchyTree } from './hierarchy-tree';
import { NodeDialog, NodeDialogData } from './node-dialog';

/**
 * View of a portfolio, program or project (mockup 05): its indicators
 * aggregated over its distinct descendant projects (planned load, consumed
 * capacity, open and late tickets), then the tree of what is under it.
 * Those who administer the node add children here.
 */
@Component({
  selector: 'app-node-page',
  imports: [
    ArchivedBanner,
    EmptyState,
    ErrorState,
    HierarchyTree,
    LocalDatePipe,
    LocalNumberPipe,
    MatButtonModule,
    MatIconModule,
    MatProgressBarModule,
    RouterLink,
    TranslocoPipe,
  ],
  templateUrl: './node-page.html',
  styleUrl: './node-page.scss',
})
export class NodePage {
  /** Id of the node, from the route. */
  readonly id = input.required<string>();

  private readonly api = inject(Api);
  private readonly auth = inject(AuthService);
  private readonly dialog = inject(MatDialog);
  private readonly tree = viewChild(HierarchyTree);

  protected readonly percent = PERCENT;
  protected readonly hoursOf = hoursOf;

  protected readonly node = resource({
    params: () => this.id(),
    loader: ({ params }) => this.api.invoke(getNode, { id: params }),
  });
  protected readonly aggregate = resource({
    params: () => this.id(),
    loader: ({ params }) => this.api.invoke(getNodeAggregate, { id: params }),
  });
  private readonly permissions = resource({
    params: () => this.id(),
    loader: ({ params }) => this.api.invoke(listMyNodePermissions, { id: params }),
  });
  /**
   * Tenant admins, and those who administer the node, add children to it;
   * nobody adds any to an archived project.
   */
  protected readonly canAddChild = computed(
    () =>
      !(this.node.hasValue() && this.node.value().archived) &&
      (this.auth.session()?.account.tenantRole === 'tenant_admin' ||
        (this.permissions.hasValue() && this.permissions.value().includes('administer_node'))),
  );

  protected ownersOf(node: Node): string {
    return node.owners.map((owner) => owner.displayName).join(', ');
  }

  protected addChild(node: Node): void {
    const data: NodeDialogData = { parent: { id: node.id, type: node.type, name: node.name } };
    this.dialog
      .open<NodeDialog, NodeDialogData, Node>(NodeDialog, { data, width: 'min(560px, 100vw)', autoFocus: 'first-tabbable' })
      .afterClosed()
      .subscribe((child) => {
        // The child shows in the tree at once; the aggregates follow within seconds.
        if (child) this.tree()?.reload();
      });
  }
}
