import { Component, computed, inject, input, resource } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatListModule } from '@angular/material/list';
import { Router, RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, Board, BoardNode, EmptyState, ErrorState, LoadingState, getNode, listBoards } from 'ui';

import { BoardSettingsData, BoardSettingsDialog, BoardSettingsOutcome } from './board-settings-dialog';

/** The boards of a node. */
interface NodeBoards {
  readonly node: BoardNode;
  readonly boards: readonly Board[];
}

/**
 * The boards the member sees (L06-US05), by node: each opens the board. A
 * new board goes on a node where the member manages boards.
 */
@Component({
  selector: 'app-boards-page',
  imports: [EmptyState, ErrorState, LoadingState, MatButtonModule, MatIconModule, MatListModule, RouterLink, TranslocoPipe],
  template: `
    <div class="header">
      <div>
        <h1 class="title">{{ 'nav.boards' | transloco }}</h1>
        @if (node() && filteredNode.hasValue()) {
          <p class="subtitle">
            {{ 'boards.of' | transloco: { name: filteredNode.value().name } }} ·
            <a routerLink="/boards">{{ 'boards.all' | transloco }}</a>
          </p>
        }
      </div>
      <button matButton="filled" type="button" class="new-board" (click)="create()">
        <mat-icon aria-hidden="true">add</mat-icon>
        {{ 'boards.new' | transloco }}
      </button>
    </div>
    @if (boards.error()) {
      <ui-error-state [error]="boards.error()" (retry)="boards.reload()" />
    } @else if (!boards.hasValue()) {
      <ui-loading-state [label]="'boards.loading' | transloco" />
    } @else {
      @for (group of groups(); track group.node.id) {
        <section class="group" [attr.aria-labelledby]="'node-' + group.node.id">
          <h2 class="node" [id]="'node-' + group.node.id">
            {{ group.node.name }} <span class="node-key">{{ group.node.key }}</span>
          </h2>
          <mat-nav-list>
            @for (board of group.boards; track board.id) {
              <a mat-list-item class="board" [routerLink]="['/boards', board.id]">
                <mat-icon matListItemIcon aria-hidden="true">view_kanban</mat-icon>
                <span matListItemTitle>{{ board.name }}</span>
                <span matListItemLine class="filter">{{ board.filter || ('boards.filterEmpty' | transloco) }}</span>
              </a>
            }
          </mat-nav-list>
        </section>
      } @empty {
        <ui-empty-state icon="view_kanban" [title]="'boards.none.title' | transloco" [message]="'boards.none.message' | transloco" />
      }
    }
  `,
  styleUrl: './boards-page.scss',
})
export class BoardsPage {
  /** `?node=`: only the boards of this node, and new ones on it. */
  readonly node = input<string | undefined>();

  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly router = inject(Router);

  protected readonly boards = resource({
    params: () => ({ nodeId: this.node() }),
    loader: ({ params }) => this.api.invoke(listBoards, params.nodeId ? { nodeId: params.nodeId } : {}),
  });
  protected readonly filteredNode = resource({
    params: () => this.node(),
    loader: ({ params }) => this.api.invoke(getNode, { id: params }),
  });
  protected readonly groups = computed<NodeBoards[]>(() => {
    if (!this.boards.hasValue()) return [];
    const groups: NodeBoards[] = [];
    for (const board of this.boards.value().items) {
      const last = groups.at(-1);
      if (last?.node.id === board.node.id) groups[groups.length - 1] = { ...last, boards: [...last.boards, board] };
      else groups.push({ node: board.node, boards: [board] });
    }
    return groups;
  });

  protected create(): void {
    this.dialog
      .open<BoardSettingsDialog, BoardSettingsData, BoardSettingsOutcome>(BoardSettingsDialog, {
        data: { nodeId: this.node() },
        width: 'min(640px, 100vw)',
        autoFocus: 'first-tabbable',
      })
      .afterClosed()
      .subscribe((outcome) => {
        if (outcome && outcome !== 'deleted') void this.router.navigate(['/boards', outcome.id]);
      });
  }
}
