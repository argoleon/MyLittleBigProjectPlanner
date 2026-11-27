import { Component, computed, input, output } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatMenuModule } from '@angular/material/menu';
import { RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import { Avatar, BoardCard as Card, BoardColumnCards, TypeChip, priorityIcon } from 'ui';

import { chipKind } from '../issues/issue-fields';

/**
 * A card of the Kanban board (mockup 04, L06-US05): key and type, title
 * (a link to the ticket), the tickets blocking it, its epic, estimate,
 * priority and assignee. A "Move" menu sends it to another column without
 * a mouse, or up and down its column for planners (L09-US01): the
 * accessible alternative to dragging it.
 */
@Component({
  selector: 'app-board-card',
  imports: [Avatar, MatButtonModule, MatIconModule, MatMenuModule, RouterLink, TranslocoPipe, TypeChip],
  template: `
    @let c = card();
    <div class="top">
      <span class="key">{{ c.key }}</span>
      <ui-type-chip [kind]="chipKind(c.type.color)" [label]="c.type.label ?? c.type.key" [iconName]="c.type.icon" />
    </div>
    <a class="summary" [routerLink]="['/issues', c.key]">{{ c.summary }}</a>
    @if (c.blockedBy.length > 0) {
      <p class="blocked">
        <mat-icon aria-hidden="true">block</mat-icon>
        {{ 'boards.card.blockedBy' | transloco: { keys: c.blockedBy.join(', ') } }}
      </p>
    }
    <div class="bottom">
      <span class="epic">
        @if (showEpic() && c.epic; as epic) {
          {{ epic.summary }}
        }
      </span>
      <mat-icon class="priority" [attr.aria-label]="'boards.card.priority' | transloco: { priority: ('priority.' + c.priority | transloco) }" role="img">
        {{ priorityIcon(c.priority) }}
      </mat-icon>
      @if (c.estimate !== null && c.estimate !== undefined) {
        <span class="estimate" [attr.aria-label]="'boards.card.estimate.' + estimation() | transloco: { value: c.estimate }">{{ c.estimate }}</span>
      }
      <ui-avatar [name]="c.assignee?.name" />
      @if (movable() && (targets().length > 0 || rankable())) {
        <button
          matIconButton
          type="button"
          class="move"
          [matMenuTriggerFor]="moveMenu"
          [disabled]="busy()"
          [attr.aria-label]="'boards.card.move' | transloco: { key: c.key }"
        >
          <mat-icon aria-hidden="true">more_vert</mat-icon>
        </button>
        <mat-menu #moveMenu="matMenu">
          @for (column of targets(); track column.id) {
            <button mat-menu-item type="button" class="move-to" (click)="moveTo.emit(column)">
              {{ 'boards.card.moveTo' | transloco: { column: column.name } }}
            </button>
          }
          @if (rankable() && !first()) {
            <button mat-menu-item type="button" class="move-up" (click)="step.emit(-1)">{{ 'boards.card.moveUp' | transloco }}</button>
          }
          @if (rankable() && !last()) {
            <button mat-menu-item type="button" class="move-down" (click)="step.emit(1)">{{ 'boards.card.moveDown' | transloco }}</button>
          }
        </mat-menu>
      }
    </div>
  `,
  styleUrl: './board-card.scss',
  host: {
    role: 'group',
    '[attr.aria-label]': 'card().key + " " + card().summary',
    '[class.busy]': 'busy()',
  },
})
export class BoardCard {
  readonly card = input.required<Card>();
  /** The columns of the board. */
  readonly columns = input.required<readonly BoardColumnCards[]>();
  /** The column it is in. */
  readonly columnId = input.required<string>();
  /** The member may move it. */
  readonly movable = input(false);
  /** Its move is under way. */
  readonly busy = input(false);
  /** The epic shows on the card (not when grouped by epic). */
  readonly showEpic = input(true);
  readonly estimation = input<'points' | 'hours'>('points');
  /** The member may reorder the cards of its column (the board's rank). */
  readonly rankable = input(false);
  /** First and last of its cell: it cannot go further up or down. */
  readonly first = input(false);
  readonly last = input(false);
  /** Moved to another column with the keyboard. */
  readonly moveTo = output<BoardColumnCards>();
  /** Moved one place up (-1) or down (1) in its column with the keyboard. */
  readonly step = output<-1 | 1>();

  protected readonly chipKind = chipKind;
  protected readonly priorityIcon = priorityIcon;
  protected readonly targets = computed(() => this.columns().filter((c) => c.id !== this.columnId()));
}
