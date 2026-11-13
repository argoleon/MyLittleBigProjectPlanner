import { NgTemplateOutlet } from '@angular/common';
import { Component, inject, input, linkedSignal, resource } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatTableModule } from '@angular/material/table';
import { RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, ErrorState, HealthChip, HierarchyNode, LocalNumberPipe, NodeType, NodeTypeChip, browseHierarchy } from 'ui';

import { PERCENT, TreeRow, canExpand, collapsed, expanded, hoursOf, levelRows, withState } from './hierarchy-rows';
import { NODE_ICONS } from './node-links';
import { VeiledName, isVeiled } from './veiled';

/**
 * The tree table of the hierarchy (mockup 05): one row per node with its
 * owners, health, progress, planned load, open and late tickets. Levels are
 * loaded when a row is expanded. A confidential project the user has no
 * access to shows veiled, with its planned load only.
 *
 * The content of the element shows when there is no row (an empty state).
 */
@Component({
  selector: 'app-hierarchy-tree',
  imports: [
    ErrorState,
    HealthChip,
    LocalNumberPipe,
    MatButtonModule,
    MatIconModule,
    MatProgressBarModule,
    MatTableModule,
    NgTemplateOutlet,
    NodeTypeChip,
    RouterLink,
    TranslocoPipe,
    VeiledName,
  ],
  templateUrl: './hierarchy-tree.html',
  styleUrl: './hierarchy-tree.scss',
})
export class HierarchyTree {
  private readonly api = inject(Api);

  /** The node whose children are the first level; without it, the top of the hierarchy. */
  readonly under = input<string>();
  /** Translated caption of the table, for screen readers. */
  readonly caption = input.required<string>();

  protected readonly columns = ['name', 'owners', 'health', 'progress', 'planned', 'open', 'late'];
  protected readonly percent = PERCENT;
  protected readonly isVeiled = isVeiled;
  protected readonly canExpand = canExpand;
  protected readonly hoursOf = hoursOf;

  protected readonly level = resource({
    params: () => ({ under: this.under() }),
    loader: ({ params }) => this.api.invoke(browseHierarchy, params),
  });
  /** The rows shown; a reload of the first level folds the tree again. */
  protected readonly rows = linkedSignal<TreeRow[]>(() => (this.level.hasValue() ? levelRows(this.level.value()) : []));

  /** Reads the first level again. */
  reload(): void {
    this.level.reload();
  }

  protected track(_index: number, row: TreeRow): string {
    return row.path;
  }

  protected iconOf(type: NodeType): string {
    return NODE_ICONS[type];
  }

  protected ownersOf(node: HierarchyNode): string {
    return node.owners.map((owner) => owner.displayName).join(', ');
  }

  protected otherParentsOf(node: HierarchyNode): string {
    return node.otherParents.map((parent) => parent.name).join(', ');
  }

  /** Folds an expanded row; loads and shows the children of another. */
  protected async toggle(row: TreeRow): Promise<void> {
    if (row.state === 'expanded') {
      this.rows.update((rows) => collapsed(rows, row.path));
      return;
    }
    this.rows.update((rows) => withState(rows, row.path, 'loading'));
    try {
      const children = await this.api.invoke(browseHierarchy, { under: row.item.id });
      this.rows.update((rows) => expanded(rows, row.path, children));
    } catch {
      this.rows.update((rows) => withState(rows, row.path, 'failed'));
    }
  }
}
