import { Component, input, output } from '@angular/core';
import { CdkDragHandle } from '@angular/cdk/drag-drop';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatMenuModule } from '@angular/material/menu';
import { RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import { Avatar, BacklogItem, BoardEstimation, LocalNumberPipe, Sprint, StatusChip, TypeChip } from 'ui';

import { chipCategory, chipKind } from '../issues/issue-fields';
import { ListId } from './backlog-model';

/** A list a ticket may be sent to: a sprint, or the backlog (`sprint` null). */
export interface Target {
  readonly id: ListId;
  readonly sprint: Sprint | null;
}

/** A move within the ticket's own list. */
export type Shift = 'up' | 'down' | 'top' | 'bottom';

/**
 * A ticket of the backlog or of a sprint (mockup 17, L09-US01): type, key (a
 * link to the ticket), title, its sub-tasks, epic, status, estimate and
 * assignee. For planners, a handle to drag it and a menu doing the same with
 * the keyboard: send it to a sprint or the backlog, or move it in its list.
 */
@Component({
  selector: 'app-backlog-row',
  imports: [
    Avatar,
    LocalNumberPipe,
    CdkDragHandle,
    MatButtonModule,
    MatIconModule,
    MatMenuModule,
    RouterLink,
    StatusChip,
    TranslocoPipe,
    TypeChip,
  ],
  templateUrl: './backlog-row.html',
  styleUrl: './backlog-row.scss',
  host: {
    role: 'listitem',
    '[attr.aria-label]': 'item().card.key + " " + item().card.summary',
    '[class.busy]': 'busy()',
    '[class.movable]': 'movable()',
  },
})
export class BacklogRow {
  readonly item = input.required<BacklogItem>();
  readonly estimation = input<BoardEstimation>('points');
  /** The member may move it: a planner, in the backlog or a sprint of this board. */
  readonly movable = input(false);
  /** Its move is under way. */
  readonly busy = input(false);
  /** Where its menu may send it. */
  readonly targets = input<readonly Target[]>([]);
  /** Its position in its list, and the list's length. */
  readonly index = input(0);
  readonly count = input(1);
  readonly sendTo = output<ListId>();
  readonly shift = output<Shift>();

  protected readonly chipCategory = chipCategory;
  protected readonly chipKind = chipKind;
}
